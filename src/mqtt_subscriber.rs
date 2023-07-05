use std::{collections::HashMap};
use tokio::{select, join, sync::{oneshot, mpsc::Sender}};
use tokio_util::sync::CancellationToken;
use log::{error, info};
use bytes::Bytes;
use rumqttc::{EventLoop, Packet};
use thiserror::Error;



use crate::{artnet_manager::ArtnetError, array_manager::DmxArrayError, defs::UniverseDefinition, messages, defs};


#[derive(Debug, Error)]
enum MqttMessageError {
    #[error("DMX topic has no subtopic")]
    MissingSubtopic,

    #[error("Invalid DMX subtopic: '{0}' (must be either Universe, Array, Effect or command")]
    InvalidSubtopic(String),

    #[error("Missing Universe ID in DMX topic: '{0}'")]
    MissingUniverseId(String),

    #[error("Missing Array ID in DMX topic: '{0}'")]
    MissingArrayId(String),

    #[error("Artnet error: {0}")]
    UniverseOperationError(#[from] ArtnetError),

    #[error("DMX Array/Values error: {0}")]
    ArrayOperationError(#[from] DmxArrayError),

    #[error("Error parsing {0} ('{1}'): {2}")]
    JsonParseError(String, String, #[source] serde_json::Error),
}

struct MqttSubscriber {
    to_artnet_tx: Sender<messages::ToArtnetManagerMessage>,
    to_array_tx: Sender<messages::ToArrayManagerMessage>,
    to_mqtt_publisher_tx: Sender<messages::ToMqttPublisherMessage>,
}

pub async fn run(cancelled: CancellationToken, mut event_loop: EventLoop, 
    to_artnet_tx: Sender<messages::ToArtnetManagerMessage>,
    to_array_tx: Sender<messages::ToArrayManagerMessage>,
    to_mqtt_publisher_tx: Sender<messages::ToMqttPublisherMessage>) {
    info!("Starting MQTT subscriber worker");

    let mqtt_subscriber = MqttSubscriber {
        to_artnet_tx,
        to_array_tx,
        to_mqtt_publisher_tx,
    };

    loop {
        select! {
            _ = cancelled.cancelled() => {
                info!("MQTT subscriber worker cancelled");
                break;
            }

            event = event_loop.poll() => {
                match event {
                    Ok(notification) => {
                        if let rumqttc::Event::Incoming(Packet::Publish(publish_packet)) = notification {
                            let topic = publish_packet.topic;
                            let payload = publish_packet.payload;
        
                            if let Err(e) = mqtt_subscriber.handle_message(&topic, &payload).await {
                                error!("Error while handling MQTT message: {:?}", e);
                                mqtt_subscriber.to_mqtt_publisher_tx.send(messages::ToMqttPublisherMessage::Error(e.to_string())).await.unwrap();
                            }
                        }
                    },

                    Err(e) => {
                        error!("Error while polling MQTT broker: {}", e);
                    }
        
                }
            }
        }
    }

    info!("Stopping MQTT subscriber worker");
}

impl MqttSubscriber {

    async fn handle_message(&self, topic: &str, payload: &Bytes) -> Result<(), MqttMessageError> {
        let topic_parts: Vec<&str> = topic.split('/').collect();

        if topic_parts.len() < 2 {
            Err(MqttMessageError::MissingSubtopic)
        }
        else {
            match topic_parts[1] {
                "Universe" => {
                    if topic_parts.len() != 3 {
                        Err(MqttMessageError::MissingUniverseId(topic_parts[1].to_string()))
                    }
                    else {
                        self.handle_universe_message(topic_parts[2], payload).await
                    }
                },
                "Array" => {
                    if topic_parts.len() != 3 {
                        Err(MqttMessageError::MissingArrayId(topic_parts[1].to_string()))
                    }
                    else {
                        self.handle_array_message(topic_parts[2], payload).await
                    }

                }
                "Values" => self.handle_values_message(payload).await,
                "Error" | "LastError" | "Active" => Ok(()),      // Ignore any message posted to Error subtopic since it is published by this service
                _ => Err(MqttMessageError::InvalidSubtopic(topic_parts[1].to_string())) 
            }
        }
    }

    async fn handle_universe_message(&self, universe_id: &str, payload: &Bytes) -> Result<(), MqttMessageError> {
        // If no payload is given, remove the universe
        if payload.is_empty() {
            let (tx_artnet_reply, rx_artnet_reply) = oneshot::channel::<Result<(), ArtnetError>>();

            self.to_artnet_tx.send(messages::ToArtnetManagerMessage::RemoveUniverse(universe_id.to_string(), tx_artnet_reply)).await.unwrap();

            if let Err(e) = rx_artnet_reply.await.unwrap() {
                return Err(MqttMessageError::UniverseOperationError(e))
            }
        }
        else {
            match serde_json::from_slice::<UniverseDefinition>(payload) {
                Ok(definition) => {
                    let (tx_artnet_reply, rx_artnet_reply) = oneshot::channel::<Result<(), ArtnetError>>();

                    self.to_artnet_tx.send(messages::ToArtnetManagerMessage::AddUniverse(universe_id.to_string(), definition, tx_artnet_reply)).await.unwrap();

                    if let Err(e) = rx_artnet_reply.await.unwrap() {
                        return Err(MqttMessageError::UniverseOperationError(e))
                    }
                },
                Err(e) => return Err(MqttMessageError::JsonParseError("universe definition".to_string(), universe_id.to_string(), e))
            }

        }

        Ok(())
    }

    async fn handle_array_message(&self, array_id: &str, payload: &Bytes) -> Result<(), MqttMessageError> {
        // If no payload is given, remove the array
        if payload.is_empty() {
            let (tx, rx) = oneshot::channel::<Result<(), DmxArrayError>>();

            self.to_array_tx.send(messages::ToArrayManagerMessage::RemoveArray(array_id.to_string(), tx)).await.unwrap();

            if let Err(e) = rx.await.unwrap() {
                return Err(MqttMessageError::ArrayOperationError(e))
            }
        }
        else {
            match serde_json::from_slice::<defs::DmxArray>(payload) {
                Ok(definition) => {
                    let (tx, rx) = oneshot::channel::<Result<(), DmxArrayError>>();

                    self.to_array_tx.send(messages::ToArrayManagerMessage::AddArray(array_id.to_string(), definition, tx)).await.unwrap();

                    if let Err(e) = rx.await.unwrap() {
                        return Err(MqttMessageError::ArrayOperationError(e))
                    }
                },
                Err(e) => return Err(MqttMessageError::JsonParseError("DMX array definition".to_string(), array_id.to_string(), e))
            }

        }

        Ok(())
    }

    async fn handle_values_message(&self, payload: &Bytes) -> Result<(), MqttMessageError> {
        if payload.is_empty() {
            let (tx, rx) = oneshot::channel::<Result<(), DmxArrayError>>();

            self.to_array_tx.send(messages::ToArrayManagerMessage::RemoveValues(tx)).await.unwrap();

            if let Err(e) = rx.await.unwrap() {
                return Err(MqttMessageError::ArrayOperationError(e))
            }
        }
        else {
            match serde_json::from_slice::<HashMap<String, String>>(payload) {
                Ok(values) => {
                    let (tx, rx) = oneshot::channel::<Result<(), DmxArrayError>>();

                    self.to_array_tx.send(messages::ToArrayManagerMessage::AddValues(values, tx)).await.unwrap();

                    if let Err(e) = rx.await.unwrap() {
                        return Err(MqttMessageError::ArrayOperationError(e))
                    }
                },
                Err(e) => return Err(MqttMessageError::JsonParseError("values definition".to_string(), "global".to_string(), e))
            }

        }

        Ok(())
    }

}
