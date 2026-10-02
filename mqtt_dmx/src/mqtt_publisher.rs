use error_stack::{ResultExt, Report};
use async_channel::Receiver;
use rumqttc::v5::{AsyncClient, mqttbytes::QoS, mqttbytes::v5::PublishProperties};
use serde::Serialize;
use std::sync::{Arc, Mutex};
use tracing::{info, warn};

use crate::{messages::ToMqttPublisherMessage, service::MqttError};

#[derive(Serialize, Debug)]
struct MqttErrorMessageBody {
    time: String,
    message: String,
}

/// Build publish properties, injecting the current traceparent if an active trace exists.
fn build_publish_properties() -> Option<PublishProperties> {
    if let Some(tp) = tracing_init::traceparent::current() {
        let mut props = PublishProperties::default();
        props.user_properties.push(("traceparent".to_string(), tp));
        Some(props)
    } else {
        None
    }
}

/// The last error report, as published on `DMX/LastError` (retained). It outlives the sessions,
/// so each new connection republishes it from the bridge's own state (no-hang F4): a broker that
/// restarted without its retained messages, or a report a reconnect cut off, gets it again.
#[derive(Default, Clone)]
pub struct LastError(Arc<Mutex<Option<Vec<u8>>>>);

impl LastError {
    /// Held to read or set the report alone.
    pub fn set(&self, report: Vec<u8>) {
        // WAIT: last-error-lock
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) = Some(report);
    }

    pub fn get(&self) -> Option<Vec<u8>> {
        // WAIT: last-error-lock
        self.0.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

pub async fn session(
    mqtt_client: AsyncClient,
    to_mqtt_publisher_rx: Receiver<ToMqttPublisherMessage>,
    last_error: LastError,
) -> Result<(), Report<MqttError>> {
    info!("Starting MQTT publisher session");
    let into_context = || MqttError::Context("In MQTT publisher session".to_string());

    loop {
        // WAIT: error-queue-recv
        match to_mqtt_publisher_rx.recv().await.change_context_lazy(into_context)? {
            ToMqttPublisherMessage::Error(error) => {
                let error_message_body = MqttErrorMessageBody {
                    time: chrono::Utc::now().to_rfc3339(),
                    message: error,
                };

                warn!(kind = "external_failure", error = ?error_message_body,
                      "MQTT DMX command error reported");

                let error_message_body = serde_json::to_vec(&error_message_body).change_context_lazy(into_context)?;
                // The bridge's state first: the next connection republishes it, whether or not
                // this publish gets out.
                last_error.set(error_message_body.clone());

                // Not the poller: these wait on rumqttc's request channel, which the pump drains
                // (no-hang §14.3).
                if let Some(props) = build_publish_properties() {
                    // WAIT: mqtt-request
                    mqtt_client.publish_with_properties("DMX/LastError", QoS::AtLeastOnce, true, error_message_body.clone(), props.clone()).await.change_context_lazy(into_context)?;
                    // WAIT: mqtt-request
                    mqtt_client.publish_with_properties("DMX/Error", QoS::AtLeastOnce, false, error_message_body, props).await.change_context_lazy(into_context)?;
                } else {
                    // WAIT: mqtt-request
                    mqtt_client.publish("DMX/LastError", QoS::AtLeastOnce, true, error_message_body.clone()).await.change_context_lazy(into_context)?;
                    // WAIT: mqtt-request
                    mqtt_client.publish("DMX/Error", QoS::AtLeastOnce, false, error_message_body).await.change_context_lazy(into_context)?;
                }
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use mqtt_test_broker::FakeBroker;
    use rumqttc::v5::MqttOptions;
    use tokio::time::Duration;

    /// An error report is published on `DMX/LastError` (retained) and `DMX/Error`. Against the
    /// in-process fake broker: a test must never reach a real one (this one once used
    /// `localhost:1883`, which on a development Mac is the live local broker).
    #[tokio::test]
    async fn test_mqtt_publisher() {
        let broker = FakeBroker::start().await;
        let mut mqtt_options = MqttOptions::new("DMX-test-publisher", "127.0.0.1", broker.port());
        mqtt_options.set_keep_alive(Duration::from_secs(5));
        let (mqtt_client, mut event_loop) = AsyncClient::new(mqtt_options, 10);

        let (to_mqtt_publisher_tx, to_mqtt_publisher_rx) = async_channel::bounded::<ToMqttPublisherMessage>(10);

        let last_error = LastError::default();
        let recorded = last_error.clone();
        tokio::spawn(async move {
            let _ = session(mqtt_client, to_mqtt_publisher_rx, last_error).await;
        });

        to_mqtt_publisher_tx.send(ToMqttPublisherMessage::Error("Test error".to_string())).await.unwrap();

        let poll = async {
            loop {
                if event_loop.poll().await.is_err() {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        };
        let arrived = async {
            broker
                .wait_until(Duration::from_secs(5), |b| {
                    !b.received_on("DMX/LastError").is_empty() && !b.received_on("DMX/Error").is_empty()
                })
                .await
        };
        let arrived = tokio::select! {
            () = poll => unreachable!(),
            arrived = arrived => arrived,
        };
        assert!(arrived, "the error report was not published on DMX/LastError and DMX/Error");
        let last = &broker.received_on("DMX/LastError")[0];
        assert!(last.retain, "DMX/LastError is not retained");
        assert!(String::from_utf8_lossy(&last.payload).contains("Test error"));
        assert_eq!(recorded.get().as_deref(), Some(&last.payload[..]), "the report published is not the one kept for the next connection");
    }
}