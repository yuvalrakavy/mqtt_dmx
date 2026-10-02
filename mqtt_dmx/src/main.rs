
mod service;
mod defs;
mod mqtt_publisher;
mod mqtt_pump;
mod mqtt_subscriber;
mod dmx;
mod artnet_manager;
mod array_manager;
//mod effects_manager;
mod messages;
mod persistence;

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

    // Keep the guard for all of main: dropping it shuts down tracing-init's OpenTelemetry providers
    // (guard.rs), so spans and OTLP logs would stop right after startup.
    let d = tracing_init::TracingInit::builder("mqtt_dmx")
        .log_to_file(true)
        .log_to_gelf_server(true)
        .file_prefix("dmx")
        .file_path("logs")
        .init()
        .unwrap();

    println!("Logging: {}", d);
    info!("Starting {}", get_version());
    info!("Logging: {}", d);

    error_stack::Report::set_color_mode(error_stack::fmt::ColorMode::None);

    let config = ServiceConfig {
        mqtt_broker_address: args.mqtt,
        storage_path,
    };

    let service = service::Service::new(config);

    // By path: the wait lint takes methods named `start` and `stop` for a dependency's.
    let service = service::Service::start(service).await;

    // WAIT: ctrl-c
    tokio::signal::ctrl_c().await.unwrap();
    let _ = service::Service::stop(service).await;
}

pub fn get_version() -> String {
    format!("mqtt_dmx: {} (built at {})", built_info::PKG_VERSION, built_info::BUILT_TIME_UTC)
}

// Include the generated-file as a separate module
pub mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}
