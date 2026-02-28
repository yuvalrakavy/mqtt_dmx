use error_stack::{Report, ResultExt};
use log::{error, info};
use rumqttc::{AsyncClient, EventLoop, LastWill, MqttOptions, QoS};
use std::{marker::PhantomData, path::PathBuf, sync::Arc};
use thiserror::Error;
use tokio::sync::mpsc::Sender;
use tokio::{task::JoinSet, time::Duration};
use tokio_util::sync::CancellationToken;

use crate::{
    array_manager,
    artnet_manager::ArtnetManager,
    get_version,
    messages::{self, ToArtnetManagerMessage},
    mqtt_publisher, mqtt_subscriber,
    persistence::Persistence,
};

pub struct Started {}
pub struct Stopped {}

pub struct ServiceConfig {
    pub mqtt_broker_address: String,
    pub storage_path: PathBuf,
}

pub struct Service<Status = Stopped> {
    config: ServiceConfig,
    cancel: Option<CancellationToken>,
    workers: JoinSet<()>,
    _status: PhantomData<Status>,
}

#[derive(Debug, Error)]
pub enum MqttError {
    #[error("DMX topic has no subtopic")]
    MissingSubtopic,

    #[error("Invalid DMX subtopic: '{0}' (must be either Universe, Array, Effect or command")]
    InvalidSubtopic(String),

    #[error("Missing Universe ID in DMX topic: '{0}'")]
    MissingUniverseId(String),

    #[error("Missing Array ID in DMX topic: '{0}'")]
    MissingArrayId(String),

    #[error("{0}")]
    Context(String),

    #[error("Error parsing {0} ('{1}'): {2}")]
    JsonParseError(Arc<str>, Arc<str>, #[source] serde_json::Error),

    #[error("Missing command (topic should be DMX/Command/[On, Off, Stop])")]
    MissingCommand,

    #[error("Invalid command: '{0}' (topic should be DMX/Command/[On, Off, Stop])")]
    InvalidCommand(String),

    #[error("Internal channel closed")]
    ChannelClosed,
}

impl Service {
    pub fn new(config: ServiceConfig) -> Service<Stopped> {
        Service {
            config,
            cancel: None,
            workers: JoinSet::new(),
            _status: PhantomData,
        }
    }

    async fn connect_to_mqtt_broker(
        mqtt_broker: &str,
    ) -> Result<(AsyncClient, EventLoop), Report<MqttError>> {
        let into_context =
            || MqttError::Context(format!("Connecting to MQTT broker {mqtt_broker}"));
        let mut mqtt_options = MqttOptions::new("DMX", mqtt_broker, 1883);
        let last_will_topic = "DMX/Active".to_string();
        let version_topic = "DMX/Version".to_string();
        let last_will = LastWill::new(&last_will_topic, "false".as_bytes(), QoS::AtLeastOnce, true);
        mqtt_options
            .set_keep_alive(Duration::from_secs(5))
            .set_last_will(last_will);

        let (mqtt_client, event_loop) = AsyncClient::new(mqtt_options, 10);

        // Publish active state
        mqtt_client
            .publish(&last_will_topic, QoS::AtLeastOnce, true, "true".as_bytes())
            .await
            .change_context_lazy(into_context)?;
        mqtt_client
            .publish(
                &version_topic,
                QoS::AtLeastOnce,
                true,
                get_version().as_bytes(),
            )
            .await
            .change_context_lazy(into_context)?;

        // Subscribe to commands
        mqtt_client
            .subscribe("DMX/#".to_string(), QoS::AtLeastOnce)
            .await
            .change_context_lazy(into_context)?;
        Ok((mqtt_client, event_loop))
    }

    async fn mqtt_session(
        broker_address: &str,
        to_artnet_tx: Sender<ToArtnetManagerMessage>,
        to_array_tx: Sender<messages::ToArrayManagerMessage>,
        to_mqtt_publisher_rx: async_channel::Receiver<messages::ToMqttPublisherMessage>,
        to_mqtt_publisher_tx: async_channel::Sender<messages::ToMqttPublisherMessage>,
        persistence: Arc<Persistence>,
    ) -> Result<(), Report<MqttError>> {
        let mut mqtt_workers = JoinSet::new();

        let (mqtt_client, mqtt_event_loop) =
            Service::connect_to_mqtt_broker(broker_address).await?;

        mqtt_workers.spawn(async move {
            let e = mqtt_publisher::session(mqtt_client, to_mqtt_publisher_rx).await;
            info!("MQTT publisher session ended: {:?}", e)
        });

        mqtt_workers.spawn(async move {
            let e = mqtt_subscriber::session(
                mqtt_event_loop,
                to_artnet_tx,
                to_array_tx,
                to_mqtt_publisher_tx,
                persistence,
            )
            .await;
            info!("MQTT subscriber session ended: {:?}", e)
        });

        let _ = mqtt_workers.join_next().await; // Wait until either the publisher or the subscriber fails
        let _ = mqtt_workers.shutdown().await; // Shutdown the other worker

        Ok(())
    }

    async fn mqtt(
        broker_address: &str,
        to_artnet_tx: Sender<ToArtnetManagerMessage>,
        to_array_tx: Sender<messages::ToArrayManagerMessage>,
        to_mqtt_publisher_rx: async_channel::Receiver<messages::ToMqttPublisherMessage>,
        to_mqtt_publisher_tx: async_channel::Sender<messages::ToMqttPublisherMessage>,
        persistence: Arc<Persistence>,
    ) {
        loop {
            let _ = Self::mqtt_session(
                    broker_address,
                    to_artnet_tx.clone(),
                    to_array_tx.clone(),
                    to_mqtt_publisher_rx.clone(),
                    to_mqtt_publisher_tx.clone(),
                    persistence.clone(),
                )
                .await;

            info!("MQTT session ended, restarting in 10 seconds");
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    }
}

impl Service<Stopped> {
    pub async fn start(mut self) -> Service<Started> {
        let cancel = CancellationToken::new();

        // Create the channels for the workers
        let (to_artnet_tx, to_artnet_rx) =
            tokio::sync::mpsc::channel::<messages::ToArtnetManagerMessage>(10);
        let (to_array_tx, to_array_rx) =
            tokio::sync::mpsc::channel::<messages::ToArrayManagerMessage>(10);
        let (to_mqtt_publisher_tx, to_mqtt_publisher_rx) =
            async_channel::bounded(10);

        let to_mqtt_publisher_tx_instance = to_mqtt_publisher_tx.clone();

        // Create Artnet manager worker
        let cancel_instance = cancel.clone();
        self.workers.spawn(async move {
            let mut artnet_manager = ArtnetManager::new();

            artnet_manager
                .run(cancel_instance, to_artnet_rx, to_mqtt_publisher_tx_instance)
                .await;
        });

        // Create array manager worker
        let cancel_instance = cancel.clone();

        self.workers.spawn(async move {
            let mut array_manager = array_manager::ArrayManager::new();

            array_manager.run(cancel_instance, to_array_rx).await;
        });

        let persistence = Arc::new(Persistence::new(self.config.storage_path.clone()));
        if let Err(e) = persistence.ensure_directory() {
            error!("Failed to create storage directory: {}", e);
        }

        // Clone channel senders for replaying persisted state
        let to_artnet_tx_replay = to_artnet_tx.clone();
        let to_array_tx_replay = to_array_tx.clone();

        // Load persisted state and replay through channels
        let persisted_universes = persistence.load_universes();
        let persisted_arrays = persistence.load_arrays();
        let persisted_effects = persistence.load_effects();
        let persisted_values = persistence.load_values();

        for (universe_id, definition) in persisted_universes {
            let (tx, rx) = tokio::sync::oneshot::channel();
            if to_artnet_tx_replay
                .send(ToArtnetManagerMessage::AddUniverse(
                    universe_id.clone(),
                    definition,
                    tx,
                ))
                .await
                .is_ok()
            {
                if let Ok(Err(e)) = rx.await {
                    error!("Failed to restore universe {}: {:?}", universe_id, e);
                }
            }
        }

        for (array_id, definition) in persisted_arrays {
            let (tx, rx) = tokio::sync::oneshot::channel();
            if to_array_tx_replay
                .send(messages::ToArrayManagerMessage::AddArray(
                    array_id.clone(),
                    Box::new(definition),
                    tx,
                ))
                .await
                .is_ok()
            {
                if let Ok(Err(e)) = rx.await {
                    error!("Failed to restore array {}: {:?}", array_id, e);
                }
            }
        }

        for (effect_id, definition) in persisted_effects {
            let (tx, rx) = tokio::sync::oneshot::channel();
            if to_array_tx_replay
                .send(messages::ToArrayManagerMessage::AddEffect(
                    effect_id.clone(),
                    definition,
                    tx,
                ))
                .await
                .is_ok()
            {
                if let Ok(Err(e)) = rx.await {
                    error!("Failed to restore effect {}: {:?}", effect_id, e);
                }
            }
        }

        for (value_name, value) in persisted_values {
            let (tx, rx) = tokio::sync::oneshot::channel();
            if to_array_tx_replay
                .send(messages::ToArrayManagerMessage::AddGlobalValue(
                    value_name.clone(),
                    Arc::from(value.as_str()),
                    tx,
                ))
                .await
                .is_ok()
            {
                if let Ok(Err(e)) = rx.await {
                    error!("Failed to restore value {}: {:?}", value_name, e);
                }
            }
        }

        info!(
            "Persisted configuration loaded from {}",
            self.config.storage_path.display()
        );

        let broker_address = self.config.mqtt_broker_address.clone();

        self.workers.spawn(async move {
            Self::mqtt(
                &broker_address,
                to_artnet_tx,
                to_array_tx,
                to_mqtt_publisher_rx,
                to_mqtt_publisher_tx,
                persistence,
            )
            .await;
        });

        info!("Service started");
        Service {
            config: self.config,
            cancel: Some(cancel),
            workers: self.workers,
            _status: PhantomData,
        }
    }
}

impl Service<Started> {
    pub async fn stop(mut self) -> Service<Stopped> {
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
        self.workers.shutdown().await;
        info!("Service stopped");

        Service {
            config: self.config,
            cancel: None,
            workers: self.workers,
            _status: PhantomData,
        }
    }
}
