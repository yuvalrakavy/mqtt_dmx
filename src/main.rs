
mod service;
mod defs;
mod mqtt_publisher;
mod mqtt_subscriber;
mod dmx;
mod artnet_manager;
mod array_manager;
//mod effects_manager;
mod messages;

use rustop::opts;
use service::ServiceConfig;

#[tokio::main]
async fn main() {
    let (args, _) = opts! {
        synopsis "MQTT DMX Controller";
        param mqtt:String, desc: "MQTT broker to connect";
    }.parse_or_exit();

    let d = tracing_init::TracingInit::builder("mqtt_ac")
        .log_to_file(true)
        .log_to_server(true)
        .log_file_prefix("ac")
        .log_file_path("logs")
        .init().map(|t| format!("{t}")).unwrap();

    println!("Logging: {}", d);

    error_stack::Report::set_color_mode(error_stack::fmt::ColorMode::None);

    let config = ServiceConfig {
        mqtt_broker_address: args.mqtt,
    };

    let service = service::Service::new(config);

    let service = service.start().await;

    tokio::signal::ctrl_c().await.unwrap();
    let _ = service.stop().await;
}

pub fn get_version() -> String {
    format!("mqtt_dmx: {} (built at {})", built_info::PKG_VERSION, built_info::BUILT_TIME_UTC)
}

// Include the generated-file as a separate module
pub mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}
