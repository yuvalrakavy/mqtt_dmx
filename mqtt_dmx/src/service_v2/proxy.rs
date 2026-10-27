use super::*;
use arc_swap::ArcSwap;
use dmx_protocol::{configuration::Configuration, Id, Playback};
use std::{io::Write, sync::Arc, time::Instant};
use tokio::sync::{mpsc, oneshot, OwnedSemaphorePermit, Semaphore};

pub(super) const QUEUE_BYTES: usize = 1024 * 1024;
pub(super) struct View {
    pub live: Option<Installation>,
    pub mode: Mode,
}
pub(super) enum Action {
    Read(Scope),
    Configure(Vec<u8>),
    Apply(String),
    Command(CommandCall),
}
pub(super) struct Request {
    pub target: Option<String>,
    pub context: RequestContext,
    pub report: Reporter,
    pub completion: Option<CommandCompletion>,
    pub action: Action,
    pub reply: oneshot::Sender<Result<Option<Value>, OpError>>,
    pub _bytes: OwnedSemaphorePermit,
}
impl Request {
    pub fn check(&self) -> Result<(), OpError> {
        if self.reply.is_closed() {
            return Err(OpError::Rejected("request caller was cancelled".into()));
        }
        if self
            .context
            .deadline()
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(OpError::Rejected(
                "original request expired before engine commit".into(),
            ));
        }
        if !self
            .report
            .request_is_current(self.target.as_deref(), &self.context)
        {
            return Err(OpError::Rejected(
                "request belongs to a retired recovery cut".into(),
            ));
        }
        Ok(())
    }
}

pub struct Proxy {
    pub(super) generation: String,
    pub(super) view: Arc<ArcSwap<View>>,
    pub(super) requests: mpsc::Sender<Request>,
    pub(super) bytes: Arc<Semaphore>,
    pub(super) reports: tokio::sync::watch::Sender<Option<Reporter>>,
    pub(super) reporter: Option<Reporter>,
}
impl Proxy {
    async fn send(
        &self,
        target: Option<&str>,
        context: &RequestContext,
        action: Action,
        cost: usize,
    ) -> Result<Option<Value>, OpError> {
        let bytes = self
            .bytes
            .clone()
            .try_acquire_many_owned(
                cost.saturating_add(512)
                    .try_into()
                    .map_err(|_| OpError::Rejected("DMX request exceeds queue budget".into()))?,
            )
            .map_err(|_| OpError::Rejected("DMX request mailbox is full".into()))?;
        let report = self
            .reporter
            .as_ref()
            .ok_or_else(|| OpError::Rejected("DMX proxy is disconnected".into()))?
            .clone();
        let completion = report.command_completion();
        let (reply, result) = oneshot::channel();
        self.requests
            .try_send(Request {
                target: target.map(str::to_owned),
                context: context.clone(),
                report,
                completion,
                action,
                reply,
                _bytes: bytes,
            })
            .map_err(|_| OpError::Rejected("DMX request mailbox is unavailable or full".into()))?;
        result.await.map_err(|_| {
            OpError::Rejected("DMX engine stopped before acknowledging the request".into())
        })?
    }
}

pub(super) fn configuration(value: Option<&Value>) -> Result<Vec<u8>, OpError> {
    struct Bounded(Vec<u8>);
    impl Write for Bounded {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.0.len().saturating_add(bytes.len())
                > dmx_protocol::configuration::MAX_CONFIG_BYTES
            {
                return Err(std::io::Error::other("configuration exceeds 128KiB"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let value = value.ok_or_else(|| {
        OpError::Rejected("configuration retraction does not reset the installation".into())
    })?;
    let mut output = Bounded(Vec::new());
    serde_json::to_writer(&mut output, value).map_err(rejected)?;
    Ok(output.0)
}
pub(super) fn rejected(error: impl std::fmt::Display) -> OpError {
    OpError::Rejected(error.to_string())
}
pub(super) fn id() -> Id {
    uuid::Uuid::new_v4()
        .to_string()
        .try_into()
        .expect("UUID generator produces canonical identities")
}
pub(super) fn playback(values: &PropertyMap) -> Result<&str, OpError> {
    if values.len() != 1 {
        return Err(rejected("only the complete playback scalar may be written"));
    }
    values
        .get("playback")
        .and_then(Value::as_str)
        .ok_or_else(|| rejected("playback must be a JSON string"))
}
pub(super) fn control(call: &CommandCall) -> Result<crate::ordered_array::Command, OpError> {
    let mut values = call.params.clone();
    if values.contains_key("restart") {
        return Err(rejected("restart is determined by the command name"));
    }
    values.insert("restart".into(), Value::Bool(call.command == "Restart"));
    serde_json::from_value(Value::Object(values)).map_err(rejected)
}
pub(super) fn administrative(name: &str) -> bool {
    matches!(name, "ReadRecovery" | "Initialize")
}
impl DeviceDriver for Proxy {
    fn info(&self) -> DriverInfo {
        DriverInfo {
            feedback: Feedback::Assumed,
            commands: [
                "Stop",
                "Restart",
                "SaveStartupFallback",
                "ReadRecovery",
                "Initialize",
            ]
            .into_iter()
            .map(|name| {
                CommandSpec::new(
                    name,
                    if name == "ReadRecovery" {
                        CommandClass::DiscoveryRead
                    } else {
                        CommandClass::WithReply
                    },
                )
            })
            .collect(),
            ..Default::default()
        }
    }
    fn request_policy(&self) -> RequestPolicy {
        RequestPolicy::Fenced(TargetGenerations {
            instance: self.generation.clone(),
            targets: BTreeMap::new(),
        })
    }
    fn validate_request(
        &self,
        target: Option<&str>,
        context: &RequestContext,
        kind: RequestKind<'_>,
    ) -> Result<(), OpError> {
        let view = self.view.load();
        if context
            .deadline()
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(rejected("original request expired"));
        }
        if let RequestKind::Command(call) = kind {
            if administrative(&call.command) {
                if target.is_some() {
                    return Err(rejected("recovery commands address the installation"));
                }
                return Ok(());
            }
        }
        if view.mode != Mode::Authoritative {
            return Err(rejected("ordinary requests require authoritative recovery"));
        }
        let live = view
            .live
            .as_ref()
            .ok_or_else(|| rejected("installation is uninitialized"))?;
        match kind {
            RequestKind::Apply(values) => {
                let array: Id = target
                    .ok_or_else(|| rejected("array target is required"))?
                    .to_owned()
                    .try_into()
                    .map_err(rejected)?;
                let scalar = playback(values)?;
                let request = Playback::decode(scalar.as_bytes()).map_err(rejected)?;
                if request.encode().map_err(rejected)? != scalar {
                    return Err(rejected("playback must use canonical encoding"));
                }
                live.validate_playback(&array, &request, context.deadline(), Instant::now())
                    .map_err(rejected)
            }
            RequestKind::Configure(value) => {
                if target != Some("configuration") {
                    return Err(rejected("configuration target is required"));
                }
                let document = configuration(value)?;
                let candidate = Configuration::decode(&document).map_err(rejected)?;
                live.validate_configuration(&candidate).map_err(rejected)
            }
            RequestKind::Command(call) if matches!(call.command.as_str(), "Stop" | "Restart") => {
                let array: Id = target
                    .ok_or_else(|| rejected("array target is required"))?
                    .to_owned()
                    .try_into()
                    .map_err(rejected)?;
                live.validate_control(&array, &control(call)?, context.deadline(), Instant::now())
                    .map_err(rejected)
            }
            RequestKind::Command(call)
                if call.command == "SaveStartupFallback" && target.is_none() =>
            {
                Ok(())
            }
            _ => Err(OpError::Unsupported("unsupported DMX request".into())),
        }
    }
    async fn connect(&mut self, context: &mut LinkContext) -> Result<(), LinkError> {
        self.reporter = Some(context.report.clone());
        self.reports.send_replace(self.reporter.clone());
        Ok(())
    }
    async fn disconnect(&mut self) {
        self.reporter = None;
    }
    async fn read_state(&mut self, scope: Scope) -> Result<Vec<TargetState>, OpError> {
        self.send(None, &RequestContext::default(), Action::Read(scope), 0)
            .await?;
        Ok(vec![])
    }
    async fn read_state_with_request_context(
        &mut self,
        scope: Scope,
        context: &RequestContext,
    ) -> Result<Vec<ContextualState>, OpError> {
        let target = match &scope {
            Scope::Target(target) => Some(target.clone()),
            _ => None,
        };
        self.send(target.as_deref(), context, Action::Read(scope), 0)
            .await?;
        Ok(vec![])
    }
    async fn apply(&mut self, _: &str, _: &PropertyMap) -> Result<(), OpError> {
        Err(rejected("request context is required"))
    }
    async fn configure(&mut self, _: &str, _: Option<&Value>) -> Result<(), OpError> {
        Err(rejected("request context is required"))
    }
    async fn execute(
        &mut self,
        _: Option<&str>,
        _: &CommandCall,
    ) -> Result<Option<Value>, OpError> {
        Err(rejected("request context is required"))
    }
    async fn apply_with_context(
        &mut self,
        target: &str,
        values: &PropertyMap,
        context: &RequestContext,
    ) -> Result<(), OpError> {
        let value = playback(values)?;
        if value.len() > dmx_protocol::MAX_PLAYBACK_BYTES {
            return Err(rejected("playback exceeds 16KiB"));
        }
        self.send(
            Some(target),
            context,
            Action::Apply(value.to_owned()),
            value.len(),
        )
        .await?;
        Ok(())
    }
    async fn configure_with_context(
        &mut self,
        target: &str,
        value: Option<&Value>,
        context: &RequestContext,
    ) -> Result<(), OpError> {
        let document = configuration(value)?;
        let cost = document.capacity();
        self.send(Some(target), context, Action::Configure(document), cost)
            .await?;
        Ok(())
    }
    async fn execute_with_context(
        &mut self,
        target: Option<&str>,
        call: &CommandCall,
        context: &RequestContext,
    ) -> Result<Option<Value>, OpError> {
        // Charge the queued decoded parameters conservatively, including JSON
        // escape expansion. Wire admission already bounds the incoming packet.
        let cost = serde_json::to_vec(&call.params)
            .map_err(rejected)?
            .len()
            .saturating_mul(4);
        if cost > QUEUE_BYTES {
            return Err(rejected("command exceeds DMX mailbox budget"));
        }
        self.send(target, context, Action::Command(call.clone()), cost)
            .await
    }
}
