use error_stack::{ResultExt, Result};
use async_channel::Receiver;
use rumqttc::AsyncClient;
use serde::Serialize;
use log::{error, info};

use crate::{messages::ToMqttPublisherMessage, service::MqttError};

#[derive(Serialize, Debug)]
struct MqttErrorMessageBody {
    time: String,
    message: String,
}

pub async fn session(mqtt_client: AsyncClient, to_mqtt_publisher_rx: Receiver<ToMqttPublisherMessage>) -> Result<(), MqttError> {
    info!("Starting MQTT publisher session");
    let into_context = || MqttError::Context("In MQTT publisher session".to_string());

    loop {
        match to_mqtt_publisher_rx.recv().await.change_context_lazy(into_context)? {
            ToMqttPublisherMessage::Error(error) => {
                let error_message_body = MqttErrorMessageBody {
                    time: chrono::Utc::now().to_rfc3339(),
                    message: error,
                };

                error!("Error: {:?}", error_message_body);

                let error_message_body = serde_json::to_vec(&error_message_body).change_context_lazy(into_context)?;

                mqtt_client.publish("DMX/LastError", rumqttc::QoS::AtLeastOnce, true, error_message_body.clone()).await.change_context_lazy(into_context)?;
                mqtt_client.publish("DMX/Error", rumqttc::QoS::AtLeastOnce, false, error_message_body).await.change_context_lazy(into_context)?;
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use tokio::time::{sleep, Duration};
    use rumqttc::{AsyncClient, MqttOptions};

    #[tokio::test]
    async fn test_mqtt_publisher() {
        let mut mqtt_options = MqttOptions::new("DMX", "localhost", 1883);
        mqtt_options.set_keep_alive(Duration::from_secs(5));
        let (mqtt_client, mut event_loop) = AsyncClient::new(mqtt_options, 10);

        let (to_mqtt_publisher_tx, to_mqtt_publisher_rx) = tokio::sync::mpsc::channel::<ToMqttPublisherMessage>(10);

        let _ = tokio::spawn(async move {
            session(mqtt_client, to_mqtt_publisher_rx).await;
        });

        to_mqtt_publisher_tx.send(ToMqttPublisherMessage::Error("Test error".to_string())).await.unwrap();

        let timeout = sleep(Duration::from_millis(500));
        tokio::pin!(timeout);

        loop {
            tokio::select! {
                _ = &mut timeout => {
                    break;
                }
                _ = event_loop.poll() => {
                }
            }
        }
    }
}