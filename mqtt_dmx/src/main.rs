
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

use tracing::info;
use rustop::opts;
use service::ServiceConfig;

#[tokio::main]
async fn main() {
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

    // Before anything starts, so a stop during startup is not lost to the signal's default action,
    // which ends the process without the service's shutdown or the log's flush.
    let mut stop = StopSignals::install();

    // Logging never stops the bridge: a destination that cannot start (an unwritable `logs`
    // directory, an unresolvable log host) is skipped, whatever logging.toml says or wherever it
    // is found (an upward search from the working directory), and should init itself fail the
    // bridge runs without logging rather than panic. Keep the guard for all of main: dropping it
    // shuts down tracing-init's OpenTelemetry providers (guard.rs), so spans and OTLP logs would
    // stop right after startup.
    let guard = match tracing_init::TracingInit::builder("mqtt_dmx")
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
    };

    info!("Starting {}", get_version());
    if let Some(guard) = &guard {
        println!("Logging: {guard}");
        info!("Logging: {guard}");
    }

    error_stack::Report::set_color_mode(error_stack::fmt::ColorMode::None);

    let config = ServiceConfig {
        mqtt_broker_address: args.mqtt,
        storage_path,
    };

    let service = service::Service::new(config);

    // By path: the wait lint takes methods named `start` and `stop` for a dependency's.
    let service = service::Service::start(service).await;

    // WAIT: stop-signal
    let signal = stop.recv().await;
    info!(signal, "Stopping");
    // Bounded (`Service::stop`).
    let _ = service::Service::stop(service).await;

    // The log's last lines out before the process ends.
    drop(guard);
}

/// SIGTERM — systemd's stop — and SIGINT (Ctrl-C): either one stops the bridge through its bounded
/// shutdown. Registered at once, by `install`, not on the first wait.
struct StopSignals {
    #[cfg(unix)]
    signals: Option<(tokio::signal::unix::Signal, tokio::signal::unix::Signal)>,
}

impl StopSignals {
    #[cfg(unix)]
    fn install() -> StopSignals {
        use tokio::signal::unix::{signal, SignalKind};
        let signals = match (signal(SignalKind::terminate()), signal(SignalKind::interrupt())) {
            (Ok(term), Ok(int)) => Some((term, int)),
            (Err(e), _) | (_, Err(e)) => {
                eprintln!("mqtt_dmx: cannot handle SIGTERM and SIGINT ({e}); they will end the process unhandled");
                None
            }
        };
        StopSignals { signals }
    }

    #[cfg(not(unix))]
    fn install() -> StopSignals {
        StopSignals {}
    }

    /// Waits for a stop signal; its name.
    #[cfg(unix)]
    async fn recv(&mut self) -> &'static str {
        match &mut self.signals {
            Some((term, int)) => {
                // WAIT: stop-signal
                tokio::select! {
                    _ = term.recv() => "SIGTERM",
                    _ = int.recv() => "SIGINT",
                }
            }
            None => {
                // WAIT: stop-signal
                std::future::pending::<()>().await;
                "none"
            }
        }
    }

    #[cfg(not(unix))]
    async fn recv(&mut self) -> &'static str {
        // WAIT: stop-signal
        let _ = tokio::signal::ctrl_c().await;
        "Ctrl-C"
    }
}

pub fn get_version() -> String {
    format!("mqtt_dmx: {} (built at {})", built_info::PKG_VERSION, built_info::BUILT_TIME_UTC)
}

// Include the generated-file as a separate module
pub mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}
