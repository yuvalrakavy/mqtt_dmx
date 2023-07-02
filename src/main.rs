
mod service;
mod defs;
mod mqtt_publisher;
mod mqtt_subscriber;
mod dmx;
mod artnet_manager;
mod array_manager;
mod effects_manager;
mod messages;

use rustop::opts;
use service::ServiceConfig;

#[tokio::main]
async fn main() {
    let (args, _) = opts! {
        synopsis "MQTT DMX Controller";
        param mqtt:String, desc: "MQTT broker to connect";
    }.parse_or_exit();

    env_logger::init();

    let config = ServiceConfig {
        mqtt_broker_address: args.mqtt,
    };

    let service = service::Service::new(config);

    let service = service.start().await;

    tokio::signal::ctrl_c().await.unwrap();
    let _ = service.stop().await;
}
