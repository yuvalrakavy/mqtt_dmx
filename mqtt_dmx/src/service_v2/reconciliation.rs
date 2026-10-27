//! Minimal setup for an installation without usable retained recovery.
//! Store does not hold or restore a second recovery snapshot.
use super::*;
pub(super) enum Action {
    Reply(Value),
    Prepare(Box<dyn FnOnce() -> Result<Prepared, OpError> + Send>),
}
impl Owner {
    pub(super) fn administrative(&mut self, request: &Request) -> Result<Action, OpError> {
        let proxy::Action::Command(call) = &request.action else {
            return Err(rejected("setup command is required"));
        };
        if request.target.is_some() {
            return Err(rejected("setup commands address the installation"));
        }
        match call.command.as_str() {
            "ReadRecovery" => {
                if !call.params.is_empty() {
                    return Err(rejected("ReadRecovery accepts no parameters"));
                }
                Ok(Action::Reply(
                    serde_json::json!({"generation":self.generations().instance,
                    "process":self.boot,"mode":format!("{:?}", self.mode)}),
                ))
            }
            "Initialize" => {
                if self.live.is_some() && self.mode != Mode::RecoveryFault {
                    return Err(rejected("Initialize requires an empty or faulted engine"));
                }
                #[derive(serde::Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Setup {
                    configuration: Value,
                }
                let setup: Setup =
                    serde_json::from_value(Value::Object(call.params.clone())).map_err(rejected)?;
                let document = proxy::configuration(Some(&setup.configuration))?;
                Ok(Action::Prepare(Box::new(move || {
                    let mut configuration =
                        dmx_protocol::configuration::Configuration::decode(&document)
                            .map_err(rejected)?;
                    configuration.history = id();
                    configuration.authority = id();
                    configuration.revision = dmx_protocol::Revision(1);
                    configuration.expected_previous = None;
                    for array in configuration.content.arrays.values_mut() {
                        array.generation = id();
                    }
                    configuration.digest = configuration.content.digest().map_err(rejected)?;
                    let executions = configuration
                        .content
                        .arrays
                        .keys()
                        .map(|array| (array.clone(), id()))
                        .collect();
                    Installation::initialize(configuration, executions)
                        .map(Prepared::Initialize)
                        .map_err(rejected)
                })))
            }
            _ => Err(rejected("unsupported setup operation")),
        }
    }
}
