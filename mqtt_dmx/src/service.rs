use error_stack::{Report, ResultExt};
use tracing::{info, warn};
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
    mqtt_outage::Outage,
    mqtt_publisher::{self, LastError},
    mqtt_pump::Pump,
    mqtt_subscriber,
    persistence::Persistence,
};

/// The bridge's liveness: `true` while it is connected, `false` from its last will.
const ACTIVE_TOPIC: &str = "DMX/Active";

/// How long after a session ends the next one starts.
const RECONNECT_AFTER: Duration = Duration::from_secs(10);

/// How long stopping the service may wait for its tasks to end.
const STOP_WITHIN: Duration = Duration::from_secs(5);

/// How long the startup may wait for the saved configuration to be read.
const LOAD_WITHIN: Duration = Duration::from_secs(10);

pub struct Started {}
pub struct Stopped {}

/// What outlives every MQTT session: the broker's outage, which spans many, the last error
/// report, which each connection republishes, and the service's stop, which the subscriber
/// consults to tell a manager that stopped with the service from one that died.
#[derive(Clone)]
struct Lasting {
    outage: Arc<Outage>,
    last_error: LastError,
    stopping: CancellationToken,
}

pub struct ServiceConfig {
    pub mqtt_broker_address: String,
    pub storage_path: PathBuf,
}

pub struct Service<Status = Stopped> {
    config: ServiceConfig,
    cancel: Option<CancellationToken>,
    workers: JoinSet<()>,
    /// The configuration's store, and its writer (`Persistence::write_saves`): apart from
    /// `workers`, so the stop can let it write what was saved before it is aborted.
    persistence: Option<Arc<Persistence>>,
    writer: Option<tokio::task::JoinHandle<()>>,
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
            persistence: None,
            writer: None,
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

    /// Subscribes to the bridge's commands, says it is active once it can hear them, and then
    /// republishes its retained model — its version and its last error report, from its own state
    /// (no-hang F4, in the fleet's order: subscribe, `Active`, the model; Store 3b review round 3,
    /// X4): every connection is a clean start, and a broker that restarted without its retained
    /// messages gets them all again. Each waits on rumqttc's request channel, which the pump drains.
    async fn announce(
        mqtt_client: &AsyncClient,
        mqtt_broker: &str,
        last_error: &LastError,
    ) -> Result<(), Report<MqttError>> {
        let into_context =
            || MqttError::Context(format!("Connecting to MQTT broker {mqtt_broker}"));

        // Build publish properties with current traceparent if a trace is active
        let props: Option<PublishProperties> = tracing_init::traceparent::current().map(|tp| {
            let mut p = PublishProperties::default();
            p.user_properties.push(("traceparent".to_string(), tp));
            p
        });

        // Subscribe to commands
        // WAIT: mqtt-request
        mqtt_client
            .subscribe("DMX/#".to_string(), QoS::AtLeastOnce)
            .await
            .change_context_lazy(into_context)?;

        publish_retained(mqtt_client, ACTIVE_TOPIC, b"true".to_vec(), &props)
            .await
            .change_context_lazy(into_context)?;
        publish_retained(mqtt_client, "DMX/Version", get_version().into_bytes(), &props)
            .await
            .change_context_lazy(into_context)?;
        if let Some(report) = last_error.get() {
            publish_retained(mqtt_client, "DMX/LastError", report, &props)
                .await
                .change_context_lazy(into_context)?;
        }
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
        lasting: Lasting,
    ) -> Result<(), Report<MqttError>> {
        let Lasting { outage, last_error, stopping } = lasting;
        let (mqtt_client, mqtt_event_loop) = Service::mqtt_client(broker_address);
        // Polling first, so every publish below has an event loop draining it.
        let (_pump, incoming) = Pump::start(mqtt_event_loop, outage);
        Service::announce(&mqtt_client, broker_address, &last_error).await?;

        let mut mqtt_workers = JoinSet::new();
        mqtt_workers.spawn(async move {
            let e = mqtt_publisher::session(mqtt_client, to_mqtt_publisher_rx, last_error).await;
            info!("MQTT publisher session ended: {:?}", e)
        });

        mqtt_workers.spawn(async move {
            let e = mqtt_subscriber::session(
                incoming,
                to_artnet_tx,
                to_array_tx,
                to_mqtt_publisher_tx,
                persistence,
                stopping,
            )
            .await;
            info!("MQTT subscriber session ended: {:?}", e)
        });

        // Until either the publisher or the subscriber ends. A failed connection ends both: the
        // pump hands the subscriber `Ended` and drops the event loop, so a publish waiting on the
        // request channel fails.
        // WAIT: mqtt-workers
        let _ = mqtt_workers.join_next().await;
        // WAIT: task-shutdown
        mqtt_workers.shutdown().await;

        Ok(())
    }

    async fn mqtt(
        broker_address: &str,
        to_artnet_tx: Sender<ToArtnetManagerMessage>,
        to_array_tx: Sender<messages::ToArrayManagerMessage>,
        to_mqtt_publisher_rx: async_channel::Receiver<messages::ToMqttPublisherMessage>,
        to_mqtt_publisher_tx: async_channel::Sender<messages::ToMqttPublisherMessage>,
        persistence: Arc<Persistence>,
        stopping: CancellationToken,
    ) {
        let lasting = Lasting { outage: Arc::new(Outage::new()), last_error: LastError::default(), stopping };
        loop {
            let _ = Self::mqtt_session(
                    broker_address,
                    to_artnet_tx.clone(),
                    to_array_tx.clone(),
                    to_mqtt_publisher_rx.clone(),
                    to_mqtt_publisher_tx.clone(),
                    persistence.clone(),
                    lasting.clone(),
                )
                .await;

            lasting.outage.session_ended(RECONNECT_AFTER);
            tokio::time::sleep(RECONNECT_AFTER).await;
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

        // Off the async workers, and bounded: a stalled disk costs the saved configuration, which
        // the broker's retained configs restore, never the startup (no-hang F1).
        let persistence = Arc::new(Persistence::new(self.config.storage_path.clone()));
        // WAIT: persistence-load
        let persisted = persistence.load(LOAD_WITHIN).await;
        self.writer = Some(tokio::spawn(Persistence::write_saves(persistence.clone())));
        self.persistence = Some(persistence.clone());

        // Clone channel senders for replaying persisted state
        let to_artnet_tx_replay = to_artnet_tx.clone();
        let to_array_tx_replay = to_array_tx.clone();

        // Replay the persisted state through the channels
        let persisted_universes = persisted.universes;
        let persisted_arrays = persisted.arrays;
        let persisted_effects = persisted.effects;
        let persisted_values = persisted.values;

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
                // WAIT: artnet-reply
                if let Ok(Err(e)) = rx.await {
                    // The file was read and parsed; what it says was refused (logging policy).
                    warn!(kind = "command_rejected", universe_id = %universe_id, error = ?e,
                          "failed to restore persisted universe: refused");
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
                // WAIT: array-reply
                if let Ok(Err(e)) = rx.await {
                    // The file was read and parsed; what it says was refused (logging policy).
                    warn!(kind = "command_rejected", array_id = %array_id, error = ?e,
                          "failed to restore persisted array: refused");
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
                // WAIT: array-reply
                if let Ok(Err(e)) = rx.await {
                    // The file was read and parsed; what it says was refused (logging policy).
                    warn!(kind = "command_rejected", effect_id = %effect_id, error = ?e,
                          "failed to restore persisted effect: refused");
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
                // WAIT: array-reply
                if let Ok(Err(e)) = rx.await {
                    // The file was read and parsed; what it says was refused (logging policy).
                    warn!(kind = "command_rejected", value_name = %value_name, error = ?e,
                          "failed to restore persisted value: refused");
                }
            }
        }

        let broker_address = self.config.mqtt_broker_address.clone();
        let cancel_mqtt = cancel.clone();

        self.workers.spawn(async move {
            Self::mqtt(
                &broker_address,
                to_artnet_tx,
                to_array_tx,
                to_mqtt_publisher_rx,
                to_mqtt_publisher_tx,
                persistence,
                cancel_mqtt,
            )
            .await;
        });

        info!("Service started");
        Service {
            config: self.config,
            cancel: Some(cancel),
            workers: self.workers,
            persistence: self.persistence,
            writer: self.writer,
            _status: PhantomData,
        }
    }
}

impl Service<Started> {
    pub async fn stop(self) -> Service<Stopped> {
        self.stop_within(STOP_WITHIN).await
    }

    /// Cancels and aborts every task, lets the writer write what was saved, and waits at most
    /// `bound` for all of it. A task inside a synchronous call cannot be aborted until it
    /// returns, and a write on a stalled disk does not return: past the bound they are left
    /// behind, with a WARN, and the stop goes on (`main` then ends the runtime within its own
    /// bound, without them).
    async fn stop_within(mut self, bound: Duration) -> Service<Stopped> {
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
        let workers = &mut self.workers;
        let persistence = self.persistence.take();
        let writer = &mut self.writer;
        let stop = async move {
            // No more saves once the MQTT session is gone.
            // WAIT: task-shutdown
            workers.shutdown().await;
            if let Some(persistence) = persistence {
                persistence.close();
            }
            if let Some(writer) = writer.as_mut() {
                // WAIT: writer-end
                let _ = writer.await;
            }
        };
        // WAIT: service-stop
        if tokio::time::timeout(bound, stop).await.is_err() {
            let writing = self.writer.as_ref().is_some_and(|w| !w.is_finished());
            warn!(kind = "shutdown_timeout", bound_ms = bound.as_millis() as u64, tasks = self.workers.len(), writing,
                  "Service stop timed out: a task or a config write did not end; stopping without it");
        }
        if let Some(writer) = self.writer.take() {
            writer.abort();
        }
        info!("Service stopped");

        Service {
            config: self.config,
            cancel: None,
            workers: self.workers,
            persistence: None,
            writer: None,
            _status: PhantomData,
        }
    }
}

/// Publishes `payload` on `topic`, retained, with `props` if a trace is active.
async fn publish_retained(
    mqtt_client: &AsyncClient,
    topic: &str,
    payload: Vec<u8>,
    props: &Option<PublishProperties>,
) -> Result<(), rumqttc::v5::ClientError> {
    match props {
        Some(p) => {
            // WAIT: mqtt-request
            mqtt_client.publish_with_properties(topic, QoS::AtLeastOnce, true, payload, p.clone()).await
        }
        None => {
            // WAIT: mqtt-request
            mqtt_client.publish(topic, QoS::AtLeastOnce, true, payload).await
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
