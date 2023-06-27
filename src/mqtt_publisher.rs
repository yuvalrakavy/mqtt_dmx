use tokio::sync::mpsc::Receiver;
use serde::Serialize;
use log::{error, info};

use crate::messages::ToMqttPublisherMessage;

#[derive(Serialize, Debug)]
struct MqttErrorMessageBody {
    time: String,
    message: String,
}

pub async fn run(mqtt_client: rumqttc::AsyncClient, mut to_mqtt_publisher_rx: Receiver<ToMqttPublisherMessage>) {
    info!("Starting MQTT publisher worker");

    while let Some(message) = to_mqtt_publisher_rx.recv().await {
        match message {
            ToMqttPublisherMessage::Error(error) => {
                let error_message_body = MqttErrorMessageBody {
                    time: chrono::Utc::now().to_rfc3339(),
                    message: error,
                };

                error!("Error: {:?}", error_message_body);

                let error_message_body = serde_json::to_vec(&error_message_body).unwrap();

                mqtt_client.publish("DMX/LastError", rumqttc::QoS::AtLeastOnce, true, error_message_body.clone()).await.unwrap();
                mqtt_client.publish("DMX/Error", rumqttc::QoS::AtLeastOnce, false, error_message_body).await.unwrap();
            }
        }
    }

    info!("Stopping MQTT publisher worker");
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
            run(mqtt_client, to_mqtt_publisher_rx).await;
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