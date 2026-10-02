use error_stack::{Report, ResultExt};
use tracing::{error, info, warn};
use rumqttc::v5::{AsyncClient, EventLoop, MqttOptions, mqttbytes::QoS, mqttbytes::v5::{LastWill, PublishProperties}};
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
    mqtt_publisher,
    mqtt_pump::Pump,
    mqtt_subscriber,
    persistence::Persistence,
};

/// The bridge's liveness: `true` while it is connected, `false` from its last will.
const ACTIVE_TOPIC: &str = "DMX/Active";

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

    /// The client and its event loop. Nothing reaches the broker until the event loop is polled.
    fn mqtt_client(mqtt_broker: &str) -> (AsyncClient, EventLoop) {
        let (host, port) = broker_host_port(mqtt_broker);
        let mut mqtt_options = MqttOptions::new("DMX", host, port);
        let last_will = LastWill::new(ACTIVE_TOPIC, "false", QoS::AtLeastOnce, true, None);
        mqtt_options
            .set_keep_alive(Duration::from_secs(5))
            .set_last_will(last_will);

        AsyncClient::new(mqtt_options, 10)
    }

    /// Publishes the bridge's active state and version, and subscribes to its commands. Each waits
    /// on rumqttc's request channel, which the pump drains.
    async fn announce(mqtt_client: &AsyncClient, mqtt_broker: &str) -> Result<(), Report<MqttError>> {
        let into_context =
            || MqttError::Context(format!("Connecting to MQTT broker {mqtt_broker}"));
        let version_topic = "DMX/Version".to_string();

        // Build publish properties with current traceparent if a trace is active
        let props: Option<PublishProperties> = tracing_init::traceparent::current().map(|tp| {
            let mut p = PublishProperties::default();
            p.user_properties.push(("traceparent".to_string(), tp));
            p
        });

        // Publish active state
        if let Some(p) = props.clone() {
            // WAIT: mqtt-request
            mqtt_client
                .publish_with_properties(ACTIVE_TOPIC, QoS::AtLeastOnce, true, "true", p)
                .await
                .change_context_lazy(into_context)?;
        } else {
            // WAIT: mqtt-request
            mqtt_client
                .publish(ACTIVE_TOPIC, QoS::AtLeastOnce, true, "true")
                .await
                .change_context_lazy(into_context)?;
        }
        if let Some(p) = props {
            // WAIT: mqtt-request
            mqtt_client
                .publish_with_properties(&version_topic, QoS::AtLeastOnce, true, get_version(), p)
                .await
                .change_context_lazy(into_context)?;
        } else {
            // WAIT: mqtt-request
            mqtt_client
                .publish(&version_topic, QoS::AtLeastOnce, true, get_version())
                .await
                .change_context_lazy(into_context)?;
        }

        // Subscribe to commands
        // WAIT: mqtt-request
        mqtt_client
            .subscribe("DMX/#".to_string(), QoS::AtLeastOnce)
            .await
            .change_context_lazy(into_context)?;
        Ok(())
    }

    /// One connection: the pump polls, the publisher publishes error reports, the subscriber
    /// handles commands. No task here polls and waits on anything else (no-hang §14.3).
    async fn mqtt_session(
        broker_address: &str,
        to_artnet_tx: Sender<ToArtnetManagerMessage>,
        to_array_tx: Sender<messages::ToArrayManagerMessage>,
        to_mqtt_publisher_rx: async_channel::Receiver<messages::ToMqttPublisherMessage>,
        to_mqtt_publisher_tx: async_channel::Sender<messages::ToMqttPublisherMessage>,
        persistence: Arc<Persistence>,
    ) -> Result<(), Report<MqttError>> {
        let (mqtt_client, mqtt_event_loop) = Service::mqtt_client(broker_address);
        // Polling first, so every publish below has an event loop draining it.
        let (_pump, incoming) = Pump::start(mqtt_event_loop);
        Service::announce(&mqtt_client, broker_address).await?;

        let mut mqtt_workers = JoinSet::new();
        mqtt_workers.spawn(async move {
            let e = mqtt_publisher::session(mqtt_client, to_mqtt_publisher_rx).await;
            info!("MQTT publisher session ended: {:?}", e)
        });

        mqtt_workers.spawn(async move {
            let e = mqtt_subscriber::session(
                incoming,
                to_artnet_tx,
                to_array_tx,
                to_mqtt_publisher_tx,
                persistence,
            )
            .await;
            info!("MQTT subscriber session ended: {:?}", e)
        });

        // Until either the publisher or the subscriber ends. A failed connection ends both: the
        // pump hands the subscriber `Ended` and drops the event loop, so a publish waiting on the
        // request channel fails.
        let _ = mqtt_workers.join_next().await; // WAIT: mqtt-workers
        mqtt_workers.shutdown().await; // WAIT: task-shutdown

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

            info!(kind = "connection_lost", "MQTT session ended, restarting in 10 seconds");
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

        // Create Artnet manager worker. The managers' loops are called by path: the wait lint
        // takes a method named `run` for a dependency's.
        let cancel_instance = cancel.clone();
        self.workers.spawn(async move {
            let mut artnet_manager = ArtnetManager::new();

            ArtnetManager::run(&mut artnet_manager, cancel_instance, to_artnet_rx, to_mqtt_publisher_tx_instance)
                .await;
        });

        // Create array manager worker
        let cancel_instance = cancel.clone();

        self.workers.spawn(async move {
            let mut array_manager = array_manager::ArrayManager::new();

            array_manager::ArrayManager::run(&mut array_manager, cancel_instance, to_array_rx).await;
        });

        let persistence = Arc::new(Persistence::new(self.config.storage_path.clone()));
        if let Err(e) = persistence.ensure_directory() {
            warn!(kind = "external_failure", path = %self.config.storage_path.display(),
                  error = %e, "failed to create storage directory");
        }

        // Clone channel senders for replaying persisted state
        let to_artnet_tx_replay = to_artnet_tx.clone();
        let to_array_tx_replay = to_array_tx.clone();

        // Load persisted state and replay through channels
        let persisted_universes = persistence.load_universes();
        let persisted_arrays = persistence.load_arrays();
        let persisted_effects = persistence.load_effects();
        let persisted_values = persistence.load_values();

        info!(
            "Loaded persisted configuration from {}: {} universes, {} arrays, {} effects, {} values",
            self.config.storage_path.display(),
            persisted_universes.len(),
            persisted_arrays.len(),
            persisted_effects.len(),
            persisted_values.len(),
        );

        for (universe_id, definition) in persisted_universes {
            let (tx, rx) = tokio::sync::oneshot::channel();
            // WAIT: artnet-queue
            if to_artnet_tx_replay
                .send(ToArtnetManagerMessage::AddUniverse(
                    universe_id.clone(),
                    definition,
                    tx,
                ))
                .await
                .is_ok()
            {
                if let Ok(Err(e)) = rx.await { // WAIT: artnet-reply
                    error!(kind = "decode_error", universe_id = %universe_id, error = ?e,
                           "failed to restore persisted universe");
                }
            }
        }

        for (array_id, definition) in persisted_arrays {
            let (tx, rx) = tokio::sync::oneshot::channel();
            // WAIT: array-queue
            if to_array_tx_replay
                .send(messages::ToArrayManagerMessage::AddArray(
                    array_id.clone(),
                    Box::new(definition),
                    tx,
                ))
                .await
                .is_ok()
            {
                if let Ok(Err(e)) = rx.await { // WAIT: array-reply
                    error!(kind = "decode_error", array_id = %array_id, error = ?e,
                           "failed to restore persisted array");
                }
            }
        }

        for (effect_id, definition) in persisted_effects {
            let (tx, rx) = tokio::sync::oneshot::channel();
            // WAIT: array-queue
            if to_array_tx_replay
                .send(messages::ToArrayManagerMessage::AddEffect(
                    effect_id.clone(),
                    definition,
                    tx,
                ))
                .await
                .is_ok()
            {
                if let Ok(Err(e)) = rx.await { // WAIT: array-reply
                    error!(kind = "decode_error", effect_id = %effect_id, error = ?e,
                           "failed to restore persisted effect");
                }
            }
        }

        for (value_name, value) in persisted_values {
            let (tx, rx) = tokio::sync::oneshot::channel();
            // WAIT: array-queue
            if to_array_tx_replay
                .send(messages::ToArrayManagerMessage::AddGlobalValue(
                    value_name.clone(),
                    Arc::from(value.as_str()),
                    tx,
                ))
                .await
                .is_ok()
            {
                if let Ok(Err(e)) = rx.await { // WAIT: array-reply
                    error!(kind = "decode_error", value_name = %value_name, error = ?e,
                           "failed to restore persisted value");
                }
            }
        }

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
        self.workers.shutdown().await; // WAIT: task-shutdown
        info!("Service stopped");

        Service {
            config: self.config,
            cancel: None,
            workers: self.workers,
            _status: PhantomData,
        }
    }
}

/// `host` or `host:port` (default 1883).
fn broker_host_port(broker: &str) -> (&str, u16) {
    match broker.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() => match port.parse() {
            Ok(port) => (host, port),
            Err(_) => (broker, 1883),
        },
        _ => (broker, 1883),
    }
}

#[cfg(test)]
mod tests;
