use std::marker::PhantomData;
use tokio::{task::JoinSet, time::Duration};
use tokio_util::sync::CancellationToken;
use rumqttc::{AsyncClient, EventLoop, MqttOptions, LastWill, QoS};
use log::info;

use crate::{messages, artnet_manager::ArtnetManager, mqtt_publisher, mqtt_subscriber, array_manager};

pub struct Started {}    
pub struct Stopped {}

pub struct ServiceConfig {
    pub mqtt_broker_address: String,
}

pub struct Service<Status = Stopped> {
    config: ServiceConfig,

    workers: JoinSet<()>,
    _status: PhantomData<Status>,
}

impl Service {
    pub fn new(config: ServiceConfig) -> Service<Stopped> {
        Service {
            config,
            workers: JoinSet::new(),
            _status: PhantomData,
        }
    }

    async fn connect_to_mqtt_broker(mqtt_broker: &str) -> (AsyncClient, EventLoop) {
        let mut mqtt_options = MqttOptions::new("DMX", mqtt_broker, 1883);
        let last_will_topic = format!("DMX/Active");
        let last_will = LastWill::new(&last_will_topic, "false".as_bytes(), QoS::AtLeastOnce, true);
        mqtt_options.set_keep_alive(Duration::from_secs(5)).set_last_will(last_will);
    
        let (mqtt_client, event_loop) = AsyncClient::new(mqtt_options, 10);

        // Publish active state
        mqtt_client.publish(&last_will_topic, QoS::AtLeastOnce, true, "true".as_bytes()).await.unwrap();

        // Subscribe to commands
        mqtt_client.subscribe(format!("DMX/#"), QoS::AtLeastOnce).await.unwrap();
        (mqtt_client, event_loop)
    }
}

impl Service<Stopped> {
    pub async fn start(mut self) -> Service<Started> {
        let cancel = CancellationToken::new();

        let (mqtt_client, mqtt_event_loop) = 
            Service::connect_to_mqtt_broker(&self.config.mqtt_broker_address).await;

        // Create the channels for the workers
        let (to_dmx_tx, to_dmx_rx) = tokio::sync::mpsc::channel::<messages::ToArtnetManagerMessage>(10);
        let (to_array_tx, to_array_rx) = tokio::sync::mpsc::channel::<messages::ToArrayManagerMessage>(10);


        // Create Artnet manager worker
        let cancel_instance = cancel.clone();
        self.workers.spawn(async move {
            let mut dmx_manager = ArtnetManager::new();

            dmx_manager.run(cancel_instance, to_dmx_rx).await;
        });

        // Create array manager worker
        let cancel_instance = cancel.clone();

        self.workers.spawn(async move {
            let mut array_manager = array_manager::ArrayManager::new();

            array_manager.run(cancel_instance, to_array_rx).await;
        });

        // Create mqtt publisher worker
        let (to_mqtt_publisher_tx, to_mqtt_publisher_rx) = tokio::sync::mpsc::channel::<messages::ToMqttPublisherMessage>(10);
        let to_mqtt_publisher_tx_instance = to_mqtt_publisher_tx.clone();

        self.workers.spawn(async move {
            mqtt_publisher::run(mqtt_client, to_mqtt_publisher_rx).await;
        });

        // Create mqtt subscriber worker
        let to_dmx_tx_instance = to_dmx_tx.clone();
        let to_array_tx_instance = to_array_tx.clone();
        let cancel_instance = cancel.clone();

        self.workers.spawn(async move {
            mqtt_subscriber::run(cancel_instance, mqtt_event_loop, to_dmx_tx_instance, to_array_tx_instance, to_mqtt_publisher_tx_instance).await;
        });

        // Create DMX refresh worker
        // self.workers.spawn(async move {
        //     polling::polling_worker(self.config.polling_period, to_dmx_tx).await;
        // });

        info!("Service started");
        Service {
            config: self.config,
            workers: self.workers,
            _status: PhantomData,
        }
    }
}

impl Service<Started> {
    pub async fn stop(mut self) -> Service<Stopped> {
        self.workers.shutdown().await;
        info!("Service stopped");

        Service {
            config: self.config,
            workers: self.workers,
            _status: PhantomData,
        }
    }
}

 
 