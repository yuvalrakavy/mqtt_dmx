//! Managed single-owner DMX service. Preparation and file writes run off-loop.
use crate::{
    installation::{Edit, EditOutcome, Installation, PreparedEdit},
    output::{DatagramSender, Frames},
    recovery_selection::{self, Cut, Decision, Mode},
};
use mqtt_bridge_kit::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
mod proxy;
mod reconciliation;
pub use proxy::Proxy;
use proxy::{id, rejected, Action, Request, View};

pub fn bridge() -> Bridge {
    Bridge::new("DMX2")
        .root("DMX")
        .app_name("mqtt_dmx2")
        .version("mqtt_dmx2 2.0.0")
        .strict_json()
        .packet_limits()
        .request_queue_bytes(proxy::QUEUE_BYTES)
        .buffers(BufferLimits {
            incoming_bytes: 1024 * 1024,
            reply_bytes: 1024 * 1024,
        })
        .request_lifetime_limit(Duration::from_secs(3600))
        .recovery(RecoveryOptions {
            target: "_recovery".into(),
            max_bytes: dmx_protocol::MAX_RECOVERY_BYTES,
        })
}
pub struct DmxService {
    fallback: PathBuf,
}
impl DmxService {
    pub fn new(fallback: PathBuf) -> Self {
        Self { fallback }
    }
}
type ResultReply = Result<Option<Value>, OpError>;
type SaveFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = (Request, ResultReply)> + Send>>;
type InstallFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>>;
struct Acknowledgement {
    connection: RecoveryConnection,
    future: InstallFuture,
    reply: Option<(Request, Value)>,
}
enum Prepared {
    Edit(PreparedEdit),
    Initialize(Installation),
}
enum Job {
    Broker(RecoveryConnection, Result<Option<Installation>, String>),
    Request(Request, Box<Result<Prepared, OpError>>),
}
struct Owner {
    live: Option<Installation>,
    frames: Option<Frames>,
    sender: DatagramSender,
    mode: Mode,
    base: Option<Cut>,
    boot: String,
    connection: Option<RecoveryConnection>,
    recovery: RecoveryHandle,
    view: Arc<arc_swap::ArcSwap<View>>,
    reporter: Option<Reporter>,
    dirty: bool,
    cause: RequestContext,
    install: Option<Acknowledgement>,
}
impl Owner {
    fn view(&self) {
        self.view.store(Arc::new(View {
            live: self.live.clone(),
            mode: self.mode,
        }));
    }
    fn generations(&self) -> TargetGenerations {
        self.live
            .as_ref()
            .filter(|_| self.mode == Mode::Authoritative)
            .map(table)
            .unwrap_or_else(|| TargetGenerations {
                instance: self.boot.clone(),
                targets: BTreeMap::new(),
            })
    }
    fn report(
        &self,
        report: &Reporter,
        scope: &Scope,
        cause: &RequestContext,
    ) -> Result<(), String> {
        if self.mode != Mode::Authoritative {
            return Ok(());
        }
        let live = self
            .live
            .as_ref()
            .ok_or("authoritative installation is absent")?;
        let generations = table(live);
        for (target, values) in states(live)? {
            if matches!(scope, Scope::Target(wanted) if wanted != &target) {
                continue;
            }
            let context = RequestContext::new(&generations.targets[&target])?
                .with_traceparent(cause.traceparent());
            report.state_with_context(&target, values, true, &context);
        }
        Ok(())
    }
    fn replicate(&mut self) -> Result<(), String> {
        if !self.dirty || self.mode != Mode::Authoritative || self.install.is_some() {
            return Ok(());
        }
        if let Some(report) = &self.reporter {
            self.report(report, &Scope::All, &self.cause)?;
        }
        if let (Some(connection), Some(live)) = (self.connection, &self.live) {
            if let Err(error) = self
                .recovery
                .publish(connection, snapshot(live, &self.cause)?)
            {
                tracing::debug!(%error, "DMX snapshot awaits a ready MQTT connection");
            }
        }
        self.dirty = false;
        Ok(())
    }
    fn output(&mut self) -> Result<(), String> {
        if let (Some(live), Some(frames)) = (&mut self.live, &mut self.frames) {
            if live.tick().map_err(|e| e.to_string())? {
                self.dirty = true;
                self.cause = RequestContext::default();
            }
            frames.render(live).map_err(|e| e.to_string())?;
            for datagram in frames.datagrams() {
                self.send(&datagram);
            }
        }
        Ok(())
    }
    fn send(&mut self, datagram: &crate::output::Datagram) {
        if let Err(error) = self.sender.send(datagram) {
            tracing::warn!(kind="external_failure", %error, "DMX output failed");
        }
    }
    fn prepared_frames(
        &self,
        next: &Installation,
    ) -> Result<(Frames, Vec<crate::output::Datagram>), String> {
        if let (Some(previous), Some(frames)) = (&self.live, &self.frames) {
            let next_channels: BTreeSet<_> = next
                .channels()
                .map(|(_, channel)| channel.clone())
                .collect();
            let released: Vec<_> = previous
                .channels()
                .map(|(_, channel)| channel)
                .filter(|channel| !next_channels.contains(*channel))
                .cloned()
                .collect();
            let mut frames = frames.clone();
            let clearing = frames
                .reconfigure(next, &released)
                .map_err(|e| e.to_string())?;
            Ok((frames, clearing))
        } else {
            Ok((Frames::new(next).map_err(|e| e.to_string())?, vec![]))
        }
    }
    fn stage(
        &mut self,
        connection: RecoveryConnection,
        candidate: Option<Installation>,
        mode: Mode,
        initiating: Option<(Request, Value)>,
    ) -> Result<(), String> {
        let cause = initiating
            .as_ref()
            .map(|(request, _)| request.context.clone())
            .unwrap_or_default();
        let (image, prepared) = if mode == Mode::Authoritative {
            let live = candidate
                .as_ref()
                .or(self.live.as_ref())
                .ok_or("recovery candidate is missing")?;
            (model(live, &cause)?, Some(self.prepared_frames(live)?))
        } else {
            (empty_image(&self.boot, &cause), None)
        };
        if let Some((request, _)) = &initiating {
            request.check().map_err(|e| e.to_string())?;
        }
        let completion = initiating
            .as_ref()
            .and_then(|(request, _)| request.completion.clone());
        let ready = if mode == Mode::Authoritative {
            RecoveryReady::Ready
        } else {
            RecoveryReady::RecoveryOnly
        };
        let ticket = self
            .recovery
            .begin_complete(connection, image, ready, completion)?;
        // All fallible preparation precedes staging. Losing MQTT after this
        // synchronous activation cannot roll lighting back.
        if let Some(candidate) = candidate {
            self.live = Some(candidate);
        }
        if let Some((frames, clearing)) = prepared {
            self.frames = Some(frames);
            for datagram in clearing {
                self.send(&datagram);
            }
        }
        self.mode = mode;
        self.cause = cause;
        self.dirty = true;
        self.view();
        self.install = Some(Acknowledgement {
            connection,
            // WAIT: v2-recovery-install
            future: Box::pin(ticket.wait()),
            reply: initiating,
        });
        Ok(())
    }
    fn installation_finished(&mut self, result: Result<(), String>) {
        let Some(ack) = self.install.take() else {
            return;
        };
        let reconcile = result.is_err() && ack.reply.is_some();
        if let Some((request, answer)) = ack.reply {
            let _ = request
                .reply
                .send(result.clone().map(|()| Some(answer)).map_err(rejected));
        }
        if let Err(error) = result {
            tracing::debug!(%error, "DMX recovery installation was not acknowledged");
        }
        if reconcile && self.mode == Mode::Authoritative {
            // Local activation was already accepted. Expiry or command cutoff
            // may reject the later runtime install; converge the model without
            // replaying the operation or borrowing a newer replay capability.
            if let Err(error) = self.stage(ack.connection, None, Mode::Authoritative, None) {
                tracing::debug!(%error, "Accepted DMX state awaits the next broker replay");
            }
        }
        self.dirty = true;
    }
    fn broker(
        &mut self,
        connection: RecoveryConnection,
        selected: Result<Option<Installation>, String>,
    ) -> Result<(), String> {
        let cut = selected.as_ref().ok().and_then(|v| v.as_ref()).map(Cut::of);
        let decision = recovery_selection::decide(
            self.mode,
            self.live.as_ref().map(Cut::of).as_ref(),
            self.base.as_ref(),
            selected
                .as_ref()
                .map(|_| cut.as_ref())
                .map_err(String::as_str),
        );
        let result = match decision {
            Decision::AdoptBroker => {
                let mut candidate = selected?.ok_or("broker snapshot disappeared")?;
                if self.mode == Mode::ProvisionalFallback {
                    candidate = candidate
                        .preserve_running_from(self.live.as_ref().ok_or("fallback disappeared")?)
                        .map_err(|e| e.to_string())?;
                }
                self.stage(connection, Some(candidate), Mode::Authoritative, None)
            }
            Decision::KeepLive | Decision::PromoteLive => {
                self.stage(connection, None, Mode::Authoritative, None)
            }
            Decision::WaitForInitialization => self.stage(connection, None, Mode::Waiting, None),
            Decision::Fault(reason) => {
                tracing::warn!(
                    kind = "recovery_fault",
                    ?reason,
                    "DMX recovery requires reconciliation; output is preserved"
                );
                self.stage(connection, None, Mode::RecoveryFault, None)
            }
        };
        if let Err(error) = result {
            tracing::debug!(%error, "DMX recovery replay was superseded");
        }
        Ok(())
    }
    fn commit(
        &mut self,
        request: Request,
        prepared: Result<Prepared, OpError>,
    ) -> Result<(), String> {
        match request.check().and(prepared) {
            Ok(Prepared::Edit(edit)) => {
                let result = (|| {
                    if self.mode != Mode::Authoritative {
                        return Err(rejected("installation is not authoritative"));
                    }
                    let live = self
                        .live
                        .as_ref()
                        .ok_or_else(|| rejected("installation is uninitialized"))?;
                    let previous = table(live);
                    let mut candidate = live.clone();
                    let outcome = edit
                        .commit(&mut candidate, Instant::now)
                        .map_err(rejected)?;
                    let (frames, clearing) = self.prepared_frames(&candidate).map_err(rejected)?;
                    request.check()?;
                    if matches!(outcome, EditOutcome::Configuration(_)) {
                        request
                            .report
                            .replace_generations(&previous, table(&candidate))
                            .map_err(rejected)?;
                    }
                    self.live = Some(candidate);
                    self.frames = Some(frames);
                    for datagram in clearing {
                        self.send(&datagram);
                    }
                    self.cause = request.context.clone();
                    self.dirty = true;
                    self.view();
                    let receipt = match outcome {
                        EditOutcome::Configuration(commit) => {
                            serde_json::json!({"revision":commit.revision})
                        }
                        EditOutcome::Playback { receipt, .. } => {
                            serde_json::to_value(receipt).map_err(rejected)?
                        }
                    };
                    Ok(Some(receipt))
                })();
                let _ = request.reply.send(result);
            }
            Ok(Prepared::Initialize(candidate)) => {
                if self.live.is_none() || self.mode == Mode::RecoveryFault {
                    let answer = serde_json::json!({"history":candidate.configuration().history,
                            "authority":candidate.configuration().authority,"generations":table(&candidate).targets,
                            "configuration":candidate.configuration()});
                    if let Some(connection) = self.connection {
                        if let Err(error) = self.stage(
                            connection,
                            Some(candidate),
                            Mode::Authoritative,
                            Some((request, answer)),
                        ) {
                            tracing::info!(%error, "Recovery command was refused before activation");
                        }
                    } else {
                        let _ = request
                            .reply
                            .send(Err(rejected("MQTT replay is not current")));
                    }
                } else {
                    let _ = request.reply.send(Err(rejected(
                        "installation was initialized while setup was prepared",
                    )));
                }
            }
            Err(error) => {
                let _ = request.reply.send(Err(error));
            }
        }
        Ok(())
    }
}

impl ProcessService for DmxService {
    type Driver = Proxy;
    async fn run(self, mut context: ServiceContext<Proxy>) -> Result<(), String> {
        let recovery = context.recovery().ok_or("recovery support is required")?;
        let mut readbacks = recovery.clone();
        let (reports_tx, mut reports) = tokio::sync::watch::channel(None);
        let (request_tx, mut requests) = mpsc::channel(32);
        let view = Arc::new(arc_swap::ArcSwap::from_pointee(View {
            live: None,
            mode: Mode::Waiting,
        }));
        let boot = uuid::Uuid::new_v4().to_string();
        context.ready(Proxy {
            generation: boot.clone(),
            view: view.clone(),
            requests: request_tx,
            bytes: Arc::new(tokio::sync::Semaphore::new(proxy::QUEUE_BYTES)),
            reports: reports_tx,
            reporter: None,
        })?;
        let mut owner = Owner {
            live: None,
            frames: None,
            sender: DatagramSender::default(),
            mode: Mode::Waiting,
            base: None,
            boot,
            connection: None,
            recovery,
            view,
            reporter: None,
            dirty: false,
            cause: RequestContext::default(),
            install: None,
        };
        let save_worker = crate::fallback_worker::SaveWorker::new(self.fallback.clone());
        let mut save: Option<SaveFuture> = None;
        let mut fallback = Some(tokio::task::spawn_blocking(move || {
            crate::recovery_file::FallbackFile::new(self.fallback)
                .load()
                .map_err(|e| e.to_string())?
                .map(|saved| {
                    Installation::restore(saved.snapshot.as_bytes()).map_err(|e| e.to_string())
                })
                .transpose()
        }));
        let mut saved = None;
        let fallback_at = Instant::now() + Duration::from_secs(5);
        let mut job: Option<tokio::task::JoinHandle<Job>> = None;
        let mut prepared = None;
        let mut pending = None;
        let mut absent = None;
        let mut ticker = tokio::time::interval(Duration::from_millis(50));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            // WAIT: v2-owner-select
            tokio::select! {
                biased;
                _ = context.cancelled() => {
                    drop(save.take()); save_worker.close();
                    if !save_worker.shutdown_until(Instant::now() + Duration::from_secs(2)).await {
                        return Err("fallback worker exceeded shutdown bound".into());
                    }
                    return Ok(());
                }
                _ = ticker.tick() => {
                    if let Some(result) = prepared.take() {
                        match result {
                            Job::Broker(connection, selected) if pending.is_none() => {
                                if selected.as_ref().is_ok_and(Option::is_none) { absent = Some(connection); }
                                owner.broker(connection, selected)?;
                            }
                            Job::Broker(..) => {},
                            Job::Request(request, result) => owner.commit(request, *result)?,
                        }
                    }
                    if matches!(owner.mode, Mode::Waiting | Mode::RecoveryFault) && owner.live.is_none() && owner.install.is_none() && Instant::now() >= fallback_at {
                        if let Some(fallback) = saved.take() {
                            owner.base = Some(Cut::of(&fallback));
                            owner.frames = Some(Frames::new(&fallback).map_err(|e| e.to_string())?);
                            owner.live = Some(fallback);
                            if owner.mode == Mode::Waiting { owner.mode = Mode::ProvisionalFallback; }
                            owner.view();
                            tracing::info!("DMX startup fallback activated provisionally");
                            if let Some(connection) = absent { owner.broker(connection, Ok(None))?; }
                        }
                    }
                    owner.output()?;
                    if save_worker.overdue(Instant::now(), Duration::from_secs(30)) { return Err("fallback worker exceeded its execution bound".into()); }
                }
                // WAIT: v2-recovery-install
                result = async { owner.install.as_mut().unwrap().future.as_mut().await }, if owner.install.is_some() => {
                    owner.installation_finished(result);
                }
                // WAIT: v2-owner-job
                result = async { job.as_mut().unwrap().await }, if job.is_some() => {
                    job = None; prepared = Some(result.map_err(|e| e.to_string())?);
                }
                // WAIT: v2-owner-job
                result = async { fallback.as_mut().unwrap().await }, if fallback.is_some() => {
                    fallback = None;
                    match result.map_err(|e| e.to_string())? {
                        Ok(candidate) => saved = candidate,
                        Err(error) => tracing::warn!(kind="fallback_unavailable", %error, "DMX startup fallback is unavailable"),
                    }
                }
                // WAIT: v2-save-result
                result = async { save.as_mut().unwrap().await }, if save.is_some() => {
                    save = None; let (request, result) = result; let _ = request.reply.send(result);
                }
                result = readbacks.readback() => {
                    let readback = result?; owner.connection = Some(readback.connection); absent = None; pending = Some(readback);
                }
                result = reports.changed() => { result.map_err(|_| "DMX proxy ended")?; owner.reporter = reports.borrow_and_update().clone(); owner.dirty = true; }
                request = requests.recv(), if job.is_none() && prepared.is_none() && pending.is_none() && owner.install.is_none() => {
                    let Some(request) = request else { return Err("DMX proxy request channel closed".into()); };
                    if let Action::Read(scope) = &request.action {
                        let result = owner.report(&request.report, scope, &request.context).map(|()| None).map_err(rejected);
                        let _ = request.reply.send(result);
                    } else if let Err(error) = request.check() { let _ = request.reply.send(Err(error)); }
                    else if let Action::Command(call) = &request.action {
                        if call.command == "SaveStartupFallback" {
                            let result = (|| {
                                if owner.mode != Mode::Authoritative { return Err(rejected("fallback save requires authoritative recovery")); }
                                if save.is_some() { return Err(rejected("another fallback save is running")); }
                                #[derive(serde::Deserialize)] #[serde(deny_unknown_fields)] struct Save { save_id: dmx_protocol::Id }
                                let args: Save = serde_json::from_value(Value::Object(call.params.clone())).map_err(rejected)?;
                                save_worker.submit(args.save_id, owner.live.as_ref().unwrap().document(), request.context.deadline()).map_err(rejected)
                            })();
                            match result {
                                Ok(ticket) => save = Some(Box::pin(async move {
                                    // WAIT: v2-save-result
                                    let result = ticket.wait().await.map(|receipt| Some(serde_json::json!({"save_id":receipt.save_id,"revision":receipt.revision,"digest":receipt.digest}))).map_err(rejected);
                                    (request, result)
                                })),
                                Err(error) => { let _ = request.reply.send(Err(error)); }
                            }
                        } else if proxy::administrative(&call.command) {
                            match owner.administrative(&request) {
                                Ok(reconciliation::Action::Reply(value)) => { let _ = request.reply.send(Ok(Some(value))); }
                                Ok(reconciliation::Action::Prepare(task)) => job = Some(tokio::task::spawn_blocking(move || Job::Request(request, Box::new(task())))),
                                Err(error) => { let _ = request.reply.send(Err(error)); }
                            }
                        } else { job = Some(prepare_edit(request, owner.live.as_ref())); }
                    } else { job = Some(prepare_edit(request, owner.live.as_ref())); }
                }
            }
            if job.is_none() && prepared.is_none() && owner.install.is_none() {
                if let Some(readback) = pending.take() {
                    job = Some(tokio::task::spawn_blocking(move || {
                        let selected = readback.document.and_then(|document| {
                            document
                                .map(|document| {
                                    #[derive(serde::Deserialize)]
                                    #[serde(deny_unknown_fields)]
                                    struct Carrier {
                                        snapshot: String,
                                    }
                                    let carrier: Carrier = dmx_protocol::decode_strict(
                                        &document,
                                        dmx_protocol::MAX_RECOVERY_BYTES,
                                    )
                                    .map_err(|e| e.to_string())?;
                                    Installation::restore(carrier.snapshot.as_bytes())
                                        .map_err(|e| e.to_string())
                                })
                                .transpose()
                        });
                        Job::Broker(readback.connection, selected)
                    }));
                }
            }
            if pending.is_none() && job.is_none() && prepared.is_none() {
                owner.replicate()?;
            }
        }
    }
}
fn prepare_edit(request: Request, live: Option<&Installation>) -> tokio::task::JoinHandle<Job> {
    let live = live.cloned();
    tokio::task::spawn_blocking(move || {
        let span = request.context.operation_span("mqtt_dmx2", "prepare");
        let _span = span.enter();
        let result = (|| {
            request.check()?;
            let live = live.ok_or_else(|| rejected("installation is uninitialized"))?;
            let edit = match &request.action {
                Action::Configure(document) => {
                    let configuration =
                        dmx_protocol::configuration::Configuration::decode(document)
                            .map_err(rejected)?;
                    Edit::Publication(configuration)
                }
                Action::Apply(scalar) => Edit::Desired {
                    array: request
                        .target
                        .clone()
                        .ok_or_else(|| rejected("array target is required"))?
                        .try_into()
                        .map_err(rejected)?,
                    playback: dmx_protocol::Playback::decode(scalar.as_bytes())
                        .map_err(rejected)?,
                    execution: id(),
                },
                Action::Command(call) if matches!(call.command.as_str(), "Stop" | "Restart") => {
                    Edit::Command {
                        array: request
                            .target
                            .clone()
                            .ok_or_else(|| rejected("array target is required"))?
                            .try_into()
                            .map_err(rejected)?,
                        command: proxy::control(call)?,
                        execution: id(),
                    }
                }
                _ => return Err(rejected("unsupported engine edit")),
            };
            live.preparation(edit, request.context.deadline())
                .run()
                .map(Prepared::Edit)
                .map_err(rejected)
        })();
        Job::Request(request, Box::new(result))
    })
}
fn table(live: &Installation) -> TargetGenerations {
    let mut targets: BTreeMap<_, _> = live
        .configuration()
        .content
        .arrays
        .iter()
        .map(|(id, array)| {
            (
                String::from(id.clone()),
                String::from(array.generation.clone()),
            )
        })
        .collect();
    targets.insert(
        "configuration".into(),
        String::from(live.configuration().authority.clone()),
    );
    TargetGenerations {
        instance: String::from(live.configuration().authority.clone()),
        targets,
    }
}
fn states(live: &Installation) -> Result<BTreeMap<String, PropertyMap>, String> {
    let mut result = BTreeMap::new();
    result.insert(
        "configuration".into(),
        [(
            "configuration".into(),
            Value::String(live.configuration().encode().map_err(|e| e.to_string())?),
        )]
        .into_iter()
        .collect(),
    );
    for (id, state) in live.states().map_err(|e| e.to_string())? {
        let Value::Object(values) = serde_json::to_value(state).map_err(|e| e.to_string())? else {
            unreachable!()
        };
        result.insert(String::from(id), values);
    }
    Ok(result)
}
fn snapshot(live: &Installation, cause: &RequestContext) -> Result<RecoverySnapshot, String> {
    let carrier = serde_json::to_vec(&serde_json::json!({"snapshot":live.document()}))
        .map_err(|e| e.to_string())?;
    RecoverySnapshot::new(live.revision().0, carrier.into(), cause.clone())
}
fn empty_image(boot: &str, cause: &RequestContext) -> RecoveryImage {
    RecoveryImage {
        context: cause.clone(),
        generations: TargetGenerations {
            instance: boot.into(),
            targets: BTreeMap::new(),
        },
        states: BTreeMap::new(),
        configurations: BTreeMap::new(),
        snapshot: None,
    }
}
fn model(live: &Installation, cause: &RequestContext) -> Result<RecoveryImage, String> {
    Ok(RecoveryImage {
        context: cause.clone(),
        generations: table(live),
        states: states(live)?,
        configurations: [(
            "configuration".into(),
            serde_json::to_value(live.configuration()).map_err(|e| e.to_string())?,
        )]
        .into(),
        snapshot: Some(snapshot(live, cause)?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::oneshot;
    struct ProbeDriver {
        boot: String,
        report: Option<oneshot::Sender<Reporter>>,
    }
    impl DeviceDriver for ProbeDriver {
        fn info(&self) -> DriverInfo {
            DriverInfo {
                feedback: Feedback::Assumed,
                ..Default::default()
            }
        }
        fn request_policy(&self) -> RequestPolicy {
            RequestPolicy::Fenced(TargetGenerations {
                instance: self.boot.clone(),
                targets: BTreeMap::new(),
            })
        }
        async fn connect(&mut self, context: &mut LinkContext) -> Result<(), LinkError> {
            if let Some(report) = self.report.take() {
                let _ = report.send(context.report.clone());
            }
            Ok(())
        }
        async fn disconnect(&mut self) {}
        async fn read_state(&mut self, _: Scope) -> Result<Vec<TargetState>, OpError> {
            Ok(vec![])
        }
        async fn apply(&mut self, _: &str, _: &PropertyMap) -> Result<(), OpError> {
            Ok(())
        }
        async fn configure(&mut self, _: &str, _: Option<&Value>) -> Result<(), OpError> {
            Ok(())
        }
        async fn execute(
            &mut self,
            _: Option<&str>,
            _: &CommandCall,
        ) -> Result<Option<Value>, OpError> {
            Ok(None)
        }
    }
    struct ProbeService {
        driver: ProbeDriver,
        handle: oneshot::Sender<RecoveryHandle>,
    }
    impl ProcessService for ProbeService {
        type Driver = ProbeDriver;
        async fn run(self, mut context: ServiceContext<ProbeDriver>) -> Result<(), String> {
            let recovery = context.recovery().unwrap();
            context.ready(self.driver)?;
            let _ = self.handle.send(recovery);
            context.cancelled().await;
            Ok(())
        }
    }

    #[tokio::test]
    async fn accepted_initialization_converges_after_late_expiry_without_borrowing_new_replay() {
        for newer_replay in [false, true] {
            let broker = mqtt_test_broker::FakeBroker::start().await;
            let boot = uuid::Uuid::new_v4().to_string();
            let (report_tx, report_rx) = oneshot::channel();
            let (handle_tx, handle_rx) = oneshot::channel();
            let running = bridge()
                .instance("probe")
                .broker(&broker.address())
                .start_managed(ProbeService {
                    driver: ProbeDriver {
                        boot: boot.clone(),
                        report: Some(report_tx),
                    },
                    handle: handle_tx,
                })
                .await
                .unwrap();
            let mut recovery = handle_rx.await.unwrap();
            let report = report_rx.await.unwrap();
            let readback = tokio::time::timeout(Duration::from_secs(2), recovery.readback())
                .await
                .unwrap()
                .unwrap();
            recovery
                .complete(
                    readback.connection,
                    empty_image(&boot, &RequestContext::default()),
                    RecoveryReady::RecoveryOnly,
                    None,
                )
                .await
                .unwrap();
            let mut owner = Owner {
                live: None,
                frames: None,
                sender: DatagramSender::default(),
                mode: Mode::Waiting,
                base: None,
                boot: boot.clone(),
                connection: Some(readback.connection),
                recovery: recovery.clone(),
                view: Arc::new(arc_swap::ArcSwap::from_pointee(View {
                    live: None,
                    mode: Mode::Waiting,
                })),
                reporter: Some(report.clone()),
                dirty: false,
                cause: RequestContext::default(),
                install: None,
            };
            let content = dmx_protocol::configuration::Content::default();
            let candidate = Installation::initialize(
                dmx_protocol::configuration::Configuration {
                    schema: 1,
                    history: id(),
                    authority: id(),
                    revision: dmx_protocol::Revision(1),
                    expected_previous: None,
                    digest: content.digest().unwrap(),
                    content,
                },
                BTreeMap::new(),
            )
            .unwrap();
            let authority = String::from(candidate.configuration().authority.clone());
            let (reply, response) = oneshot::channel();
            let permit = Arc::new(tokio::sync::Semaphore::new(1))
                .acquire_owned()
                .await
                .unwrap();
            let deadline = Instant::now() + Duration::from_millis(50);
            let request = Request {
                target: None,
                context: RequestContext::new(&boot)
                    .unwrap()
                    .with_deadline(Some(deadline)),
                report: report.clone(),
                completion: None,
                action: Action::Command(CommandCall {
                    command: "Initialize".into(),
                    params: PropertyMap::new(),
                }),
                reply,
                _bytes: permit,
            };
            owner
                .stage(
                    readback.connection,
                    Some(candidate),
                    Mode::Authoritative,
                    Some((request, Value::Null)),
                )
                .unwrap();
            // SDK tasks share this test's current-thread runtime. Delay yielding
            // until the staged request expires, deterministically after local
            // activation and before the runtime's final deadline check.
            std::thread::sleep(Duration::from_millis(70));
            let failed = tokio::time::timeout(
                Duration::from_secs(2),
                owner.install.as_mut().unwrap().future.as_mut(),
            )
            .await
            .unwrap();
            assert!(failed.as_ref().unwrap_err().contains("expired"));
            assert_eq!(owner.mode, Mode::Authoritative);
            assert!(!report.request_is_current(None, &RequestContext::new(&authority).unwrap()));
            if newer_replay {
                broker.drop_connection();
                let next = tokio::time::timeout(Duration::from_secs(3), recovery.readback())
                    .await
                    .unwrap()
                    .unwrap();
                assert_ne!(next.connection, readback.connection);
                owner.connection = Some(next.connection);
            }
            owner.installation_finished(failed);
            assert!(
                response.await.unwrap().is_err(),
                "failed command outcome was reported as success"
            );
            assert_eq!(
                String::from(
                    owner
                        .live
                        .as_ref()
                        .unwrap()
                        .configuration()
                        .authority
                        .clone()
                ),
                authority
            );
            if newer_replay {
                assert!(
                    owner.install.is_none(),
                    "repair borrowed a newer replay capability"
                );
                assert!(!report.request_is_current(None, &RequestContext::new(&authority).unwrap()));
            } else {
                assert!(
                    owner.install.is_some(),
                    "accepted lighting was left stranded behind the old runtime model"
                );
                let repaired = tokio::time::timeout(
                    Duration::from_secs(2),
                    owner.install.as_mut().unwrap().future.as_mut(),
                )
                .await
                .unwrap();
                owner.installation_finished(repaired.clone());
                repaired.unwrap();
                assert!(report.request_is_current(None, &RequestContext::new(&authority).unwrap()));
                assert!(!report.request_is_current(None, &RequestContext::new(&boot).unwrap()));
            }
            assert!(running.stop().await);
        }
    }
}
