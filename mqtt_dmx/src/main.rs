
mod service;
mod defs;
mod mqtt_outage;
mod mqtt_publisher;
mod mqtt_pump;
mod mqtt_subscriber;
mod dmx;
mod artnet_manager;
mod array_manager;
//mod effects_manager;
mod messages;
mod persistence;
#[cfg(test)]
mod test_log;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use tracing::{info, warn};
use rustop::opts;
use service::ServiceConfig;

/// How long the runtime may take to end once the service has stopped. Every task has been
/// aborted by then; what the runtime would still wait for is a thread inside a synchronous call —
/// a write to a stalled disk, a name lookup — which nothing can interrupt, and which a dropped
/// runtime waits for without limit (no-hang F1, Store 3b re-review X1).
const RUNTIME_DRAIN: Duration = Duration::from_secs(1);

/// How long the logging may take to start before the bridge goes on without it: past tracing-init's
/// own bounds on its destinations' starts (5 s each, the file's and GELF's).
const LOGGING_START: Duration = Duration::from_secs(15);

fn main() -> ExitCode {
    let (args, _) = opts! {
        synopsis "MQTT DMX Controller";
        param mqtt:String, desc: "MQTT broker to connect";
        opt storage:Option<String>, desc: "Path to config storage directory";
    }.parse_or_exit();

    // Resolve storage path: CLI > env var > default
    let storage_path = PathBuf::from(
        args.storage
            .or_else(|| std::env::var("MQTT_DMX_STORAGE_PATH").ok())
            .unwrap_or_else(|| "dmx_config".to_string()),
    );

    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(e) => {
            // Before any stop handler, so a SIGTERM still ends the process should this write block
            // (no-hang B2's exemption: nothing here is bounded yet, and nothing needs to be).
            eprintln!("mqtt_dmx: cannot start the async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    // WAIT: main-run
    let guard = runtime.block_on(run(args.mqtt, storage_path));
    // Bounded, where dropping the runtime is not: a thread stuck in a synchronous call is left
    // behind, and ends with the process.
    runtime.shutdown_timeout(RUNTIME_DRAIN);
    // The log's last lines out before the process ends.
    drop(guard);
    ExitCode::SUCCESS
}

/// The bridge, from its stop signals to its stopped service; the log's guard, for `main` to drop
/// last.
async fn run(mqtt_broker_address: String, storage_path: PathBuf) -> Option<tracing_init::TracingGuard> {
    // Before anything starts, so a stop during startup is not lost to the signal's default action,
    // which ends the process without the service's shutdown or the log's flush.
    let mut stop = StopSignals::install();

    // Starting the logging reads its configuration, opens its file and resolves its log host: the
    // filesystem and a name lookup, which can stall (a hung disk or mount, a resolver that does not
    // answer). tracing-init bounds each destination's start at 5 s, not its configuration's read;
    // the whole start runs on a blocking thread, raced against the stop (no-hang F3, Store 3b review
    // round 3, B1), under a bound past which the bridge goes on without a log — logging never stops
    // the bridge. A thread stuck there is left to the runtime's bounded end (`main-run`); should it
    // finish later, its guard is dropped with it, which stops the log.
    // WAIT: logging-start
    let guard = tokio::select! {
        started = tokio::time::timeout(LOGGING_START, tokio::task::spawn_blocking(start_logging)) => match started {
            Ok(Ok(guard)) => guard,
            // The start panicked, or is still stuck: no log to say so in.
            Ok(Err(_)) | Err(_) => None,
        },
        // Before there is a log: nothing to say it in (B2: never straight to stdout or stderr).
        _ = stop.recv() => return None,
    };

    info!("Starting {}", get_version());
    if let Some(guard) = &guard {
        // Through the log alone: its writers drop lines rather than wait, where stdout, a
        // supervisor's pipe that has stopped draining, would block (Store 3b review round 3, B2).
        info!("Logging: {guard}");
    }
    // Now that there is a log to say it in (no-hang F3).
    stop.report_unavailable();

    error_stack::Report::set_color_mode(error_stack::fmt::ColorMode::None);

    let config = ServiceConfig {
        mqtt_broker_address,
        storage_path,
    };

    let service = service::Service::new(config);

    // A stop during the startup ends it where it is: the startup's waits are bounded, but a stop
    // need not sit them out. What it leaves running is aborted with the runtime. By path: the
    // wait lint takes methods named `start` and `stop` for a dependency's.
    // WAIT: stop-signal
    let service = tokio::select! {
        service = service::Service::start(service) => service,
        signal = stop.recv() => {
            info!(signal, "Stopping during startup");
            return guard;
        }
    };

    // WAIT: stop-signal
    let signal = stop.recv().await;
    info!(signal, "Stopping");
    // Bounded (`Service::stop`).
    let _ = service::Service::stop(service).await;
    guard
}

/// The logging, on the blocking thread `run` races against the stop. It never stops the bridge: a
/// destination that cannot start (an unwritable `logs` directory, an unresolvable log host, either
/// stuck past tracing-init's 5 s) is skipped, whatever logging.toml says or wherever it is found (an
/// upward search from the working directory), and should init itself fail the bridge runs without
/// logging rather than panic. That failure is said on stderr from here, the one message before
/// there is a log: a stalled stderr then holds this thread alone, which the stop does not wait for.
/// The guard is kept for all of main: dropping it shuts down tracing-init's writers and
/// OpenTelemetry providers (guard.rs), so the log would stop.
fn start_logging() -> Option<tracing_init::TracingGuard> {
    logging_gate();
    match tracing_init::TracingInit::builder("mqtt_dmx")
        .log_to_file(true)
        .log_to_gelf_server(true)
        .file_prefix("dmx")
        .file_path("logs")
        .on_destination_error(tracing_init::types::OnDestinationError::Skip)
        .init()
    {
        Ok(guard) => Some(guard),
        Err(e) => {
            eprintln!("mqtt_dmx: logging did not start ({e}); running without it");
            None
        }
    }
}

/// A test seam, in debug builds only: `MQTT_DMX_TEST_LOGGING_GATE` names a file the logging start reads before
/// anything else, so a process test can hold the start — a FIFO it keeps open — where it can prove
/// the start began and is still waiting, and that a stop ends the bridge anyway (Store no-hang 3b
/// review round 3, B1). Inert unless the variable is set; compiled out of release builds.
#[cfg(debug_assertions)]
fn logging_gate() {
    if let Some(gate) = std::env::var_os("MQTT_DMX_TEST_LOGGING_GATE") {
        let _ = std::fs::read(gate);
    }
}

#[cfg(not(debug_assertions))]
fn logging_gate() {}

/// SIGTERM — systemd's stop — and SIGINT (Ctrl-C): either one stops the bridge through its bounded
/// shutdown. Registered at once, by `install`, not on the first wait.
struct StopSignals {
    #[cfg(unix)]
    term: Option<tokio::signal::unix::Signal>,
    #[cfg(unix)]
    int: Option<tokio::signal::unix::Signal>,
    /// The signals whose handler could not be registered, and why: logged once logging is up.
    unavailable: Vec<(&'static str, String)>,
}

impl StopSignals {
    #[cfg(unix)]
    fn install() -> StopSignals {
        use tokio::signal::unix::{signal, SignalKind};
        let mut unavailable = Vec::new();
        let mut register = |name: &'static str, kind: SignalKind| match signal(kind) {
            Ok(signal) => Some(signal),
            Err(e) => {
                unavailable.push((name, e.to_string()));
                None
            }
        };
        let term = register("SIGTERM", SignalKind::terminate());
        let int = register("SIGINT", SignalKind::interrupt());
        StopSignals { term, int, unavailable }
    }

    #[cfg(not(unix))]
    fn install() -> StopSignals {
        StopSignals { unavailable: Vec::new() }
    }

    /// A WARN for each signal whose handler could not be registered: that signal ends the process
    /// by its default action, skipping the bounded shutdown and the log's flush.
    fn report_unavailable(&self) {
        for (signal, error) in &self.unavailable {
            warn!(kind = "signal_handler_unavailable", signal, error = %error,
                  "Stop signal handler unavailable: this signal will end the bridge without its shutdown");
        }
    }

    /// Waits for a stop signal; its name. One that could not be registered never comes.
    #[cfg(unix)]
    async fn recv(&mut self) -> &'static str {
        // WAIT: stop-signal
        tokio::select! {
            Some(()) = recv_or_never(&mut self.term) => "SIGTERM",
            Some(()) = recv_or_never(&mut self.int) => "SIGINT",
            // Both streams ended: the runtime's signal driver is gone.
            else => "signals closed",
        }
    }

    #[cfg(not(unix))]
    async fn recv(&mut self) -> &'static str {
        // WAIT: stop-signal
        let _ = tokio::signal::ctrl_c().await;
        "Ctrl-C"
    }
}

/// The next delivery of `signal`, or never if it has no handler.
#[cfg(unix)]
async fn recv_or_never(signal: &mut Option<tokio::signal::unix::Signal>) -> Option<()> {
    match signal {
        // WAIT: stop-signal
        Some(signal) => signal.recv().await,
        None => {
            // WAIT: stop-signal
            std::future::pending::<()>().await;
            None
        }
    }
}

pub fn get_version() -> String {
    format!("mqtt_dmx: {} (built at {})", built_info::PKG_VERSION, built_info::BUILT_TIME_UTC)
}

// Include the generated-file as a separate module
pub mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

#[cfg(test)]
mod tests {
    use super::StopSignals;
    use crate::test_log::capture;

    /// A stop signal whose handler could not be registered is a WARN of its own kind once logging
    /// is up — never only a line on stderr, nor an `external_failure` (no-hang F3, Store 3b
    /// re-review X3).
    #[test]
    fn a_signal_without_a_handler_is_a_warn_once_logging_is_up() {
        let signals = StopSignals {
            #[cfg(unix)]
            term: None,
            #[cfg(unix)]
            int: None,
            unavailable: vec![("SIGTERM", "no reactor".to_string())],
        };
        let events = capture(|| signals.report_unavailable());
        assert!(
            events.iter().any(|e| e.level == tracing::Level::WARN
                && e.kind() == Some("signal_handler_unavailable")
                && e.field("signal") == Some("SIGTERM")),
            "no signal_handler_unavailable WARN naming the signal: {events:#?}"
        );
    }
}
