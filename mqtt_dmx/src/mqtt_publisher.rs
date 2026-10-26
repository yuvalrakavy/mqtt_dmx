use error_stack::{ResultExt, Report};
use async_channel::Receiver;
use rumqttc::v5::{AsyncClient, mqttbytes::QoS, mqttbytes::v5::PublishProperties};
use serde::Serialize;
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

pub async fn session(mqtt_client: AsyncClient, to_mqtt_publisher_rx: Receiver<ToMqttPublisherMessage>) -> Result<(), Report<MqttError>> {
    info!("Starting MQTT publisher session");
    let into_context = || MqttError::Context("In MQTT publisher session".to_string());

    loop {
        match to_mqtt_publisher_rx.recv().await.change_context_lazy(into_context)? {
            ToMqttPublisherMessage::Error(error) => {
                let error_message_body = MqttErrorMessageBody {
                    time: chrono::Utc::now().to_rfc3339(),
                    message: error,
                };

                warn!(kind = "external_failure", error = ?error_message_body,
                      "MQTT DMX command error reported");

                let error_message_body = serde_json::to_vec(&error_message_body).change_context_lazy(into_context)?;

                if let Some(props) = build_publish_properties() {
                    mqtt_client.publish_with_properties("DMX/LastError", QoS::AtLeastOnce, true, error_message_body.clone(), props.clone()).await.change_context_lazy(into_context)?;
                    mqtt_client.publish_with_properties("DMX/Error", QoS::AtLeastOnce, false, error_message_body, props).await.change_context_lazy(into_context)?;
                } else {
                    mqtt_client.publish("DMX/LastError", QoS::AtLeastOnce, true, error_message_body.clone()).await.change_context_lazy(into_context)?;
                    mqtt_client.publish("DMX/Error", QoS::AtLeastOnce, false, error_message_body).await.change_context_lazy(into_context)?;
                }
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use rumqttc::v5::MqttOptions;
    use mqtt_test_broker::FakeBroker;
    use tokio::time::Duration;

    #[tokio::test]
    async fn test_mqtt_publisher() {
        let broker = FakeBroker::start().await;
        let mut mqtt_options = MqttOptions::new("dmx-publisher-test", "127.0.0.1", broker.port());
        mqtt_options.set_keep_alive(Duration::from_secs(5));
        let (mqtt_client, mut event_loop) = AsyncClient::new(mqtt_options, 10);

        let (to_mqtt_publisher_tx, to_mqtt_publisher_rx) = async_channel::bounded::<ToMqttPublisherMessage>(10);

        let publisher = tokio::spawn(async move {
            let _ = session(mqtt_client, to_mqtt_publisher_rx).await;
        });

        let pump = tokio::spawn(async move {
            while event_loop.poll().await.is_ok() {}
        });

        to_mqtt_publisher_tx.send(ToMqttPublisherMessage::Error("Test error".to_string())).await.unwrap();

        let received = broker.wait_until(Duration::from_secs(2), |b| {
            !b.received_on("DMX/LastError").is_empty() && !b.received_on("DMX/Error").is_empty()
        }).await;
        publisher.abort();
        pump.abort();
        let _ = publisher.await;
        let _ = pump.await;
        assert!(received, "publisher did not send both error messages to the isolated broker");
        for (topic, retained) in [("DMX/LastError", true), ("DMX/Error", false)] {
            let messages = broker.received_on(topic);
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0].retain, retained);
            let payload: serde_json::Value = serde_json::from_slice(&messages[0].payload).unwrap();
            assert_eq!(payload["message"], "Test error");
            assert!(payload["time"].as_str().is_some_and(|time| !time.is_empty()));
        }
    }
}
