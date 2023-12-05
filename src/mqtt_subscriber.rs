use std::sync::Arc;
use error_stack::{Result, ResultExt};

use bytes::Bytes;
use log::{error, info};
use rumqttc::{EventLoop, Packet};
use thiserror::Error;
use tokio::{
    select,
    sync::{mpsc::Sender, oneshot},
};
use tokio_util::sync::CancellationToken;

use crate::{
    array_manager::DmxArrayError,
    artnet_manager::{ArtnetError, EffectNodeRuntime},
    defs::{self, EffectNodeDefinition, DIMMING_AMOUNT_MAX},
    defs::{EffectUsage, UniverseDefinition},
    messages,
};

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

    #[error("While: {0}")]
    Context(String),

    #[error("Error parsing {0} ('{1}'): {2}")]
    JsonParseError(Arc<str>, Arc<str>, #[source] serde_json::Error),

    #[error("Missing command (topic should be DMX/Command/[On, Off, Stop])")]
    MissingCommand,

    #[error("Invalid command: '{0}' (topic should be DMX/Command/[On, Off, Stop])")]
    InvalidCommand(String),
}

struct MqttSubscriber {
    to_artnet_tx: Sender<messages::ToArtnetManagerMessage>,
    to_array_tx: Sender<messages::ToArrayManagerMessage>,
    to_mqtt_publisher_tx: Sender<messages::ToMqttPublisherMessage>,
}

pub async fn run(
    cancelled: CancellationToken,
    mut event_loop: EventLoop,
    to_artnet_tx: Sender<messages::ToArtnetManagerMessage>,
    to_array_tx: Sender<messages::ToArrayManagerMessage>,
    to_mqtt_publisher_tx: Sender<messages::ToMqttPublisherMessage>,
) {
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
            Err(MqttMessageError::MissingSubtopic.into())
        } else {
            match topic_parts[1] {
                "Universe" => {
                    if topic_parts.len() != 3 {
                        Err(MqttMessageError::MissingUniverseId(
                            topic_parts[1].to_string(),
                        ).into())
                    } else {
                        self.handle_universe_message(Arc::from(topic_parts[2]), payload).await
                    }
                }
                "Array" => {
                    if topic_parts.len() != 3 {
                        Err(MqttMessageError::MissingArrayId(topic_parts[1].to_string()).into())
                    } else {
                        self.handle_array_message(Arc::from(topic_parts[2]), payload).await
                    }
                }
                "Command" => {
                    if topic_parts.len() != 3 {
                        Err(MqttMessageError::MissingCommand.into())
                    } else {
                        self.handle_command_message(Arc::from(topic_parts[2]), payload).await
                    }
                }
                "Value" => {
                    if topic_parts.len() != 3 {
                        Err(MqttMessageError::MissingCommand.into())
                    } else {
                        self.handle_value_message(Arc::from(topic_parts[2]), payload).await
                    }
                }
                "Effect" => {
                    if topic_parts.len() != 3 {
                        Err(MqttMessageError::MissingCommand.into())
                    } else {
                        self.handle_effect_message(Arc::from(topic_parts[2]), payload).await
                    }
                }
                "Error" | "LastError" | "Active" => Ok(()), // Ignore any message posted to Error subtopic since it is published by this service
                _ => Err(MqttMessageError::InvalidSubtopic(
                    topic_parts[1].to_string(),
                ).into()),
            }
        }
    }

    async fn handle_universe_message(
        &self,
        universe_id: Arc<str>,
        payload: &Bytes,
    ) -> Result<(), MqttMessageError> {
        // If no payload is given, remove the universe
        if payload.is_empty() {
            let (tx_artnet_reply, rx_artnet_reply) = oneshot::channel::<Result<(), ArtnetError>>();

            self.to_artnet_tx
                .send(messages::ToArtnetManagerMessage::RemoveUniverse(
                    universe_id,
                    tx_artnet_reply,
                ))
                .await
                .unwrap();

            if let Err(e) = rx_artnet_reply.await.unwrap() {
                return Err(e).change_context_lazy(|| MqttMessageError::Context(String::from("removing universe")));
            }
        } else {
            match serde_json::from_slice::<UniverseDefinition>(payload) {
                Ok(definition) => {
                    let (tx_artnet_reply, rx_artnet_reply) =
                        oneshot::channel::<Result<(), ArtnetError>>();

                    self.to_artnet_tx
                        .send(messages::ToArtnetManagerMessage::AddUniverse(
                            universe_id.clone(),
                            definition,
                            tx_artnet_reply,
                        ))
                        .await
                        .unwrap();

                    if let Err(e) = rx_artnet_reply.await.unwrap() {
                        return Err(e).change_context_lazy(|| MqttMessageError::Context(format!("adding universe {universe_id}")));
                    }
                }
                Err(e) => {
                    return Err(MqttMessageError::JsonParseError(
                        Arc::from("universe definition"),
                        universe_id.clone(),
                        e,
                    )).change_context_lazy(|| MqttMessageError::Context(format!("parsing universe definition {universe_id}")));
                }
            }
        }

        Ok(())
    }

    async fn handle_array_message(
        &self,
        array_id: Arc<str>,
        payload: &Bytes,
    ) -> Result<(), MqttMessageError> {
        // If no payload is given, remove the array
        if payload.is_empty() {
            let (tx, rx) = oneshot::channel::<Result<(), DmxArrayError>>();

            self.to_array_tx
                .send(messages::ToArrayManagerMessage::RemoveArray(
                    array_id.clone(),
                    tx,
                ))
                .await
                .unwrap();

            if let Err(e) = rx.await.unwrap() {
                return Err(e).change_context_lazy(|| MqttMessageError::Context(format!("removing array {array_id}")));
            }
        } else {
            let into_context = || MqttMessageError::Context(format!("adding array {array_id}"));

            match serde_json::from_slice::<defs::DmxArray>(payload) {
                Ok(definition) => {

                    let (tx, rx) = oneshot::channel::<Result<(), DmxArrayError>>();

                    self.to_array_tx
                        .send(messages::ToArrayManagerMessage::AddArray(
                            array_id.clone(),
                            Box::new(definition),
                            tx,
                        ))
                        .await
                        .unwrap();

                    if let Err(e) = rx.await.unwrap() {
                        return Err(e).change_context_lazy(into_context);
                    }
                }
                Err(e) => {
                    return Err(e).change_context_lazy(into_context)
                }
            }
        }

        Ok(())
    }

    async fn handle_value_message(
        &self,
        value_name: Arc<str>,
        payload: &Bytes,
    ) -> Result<(), MqttMessageError> {
        if payload.is_empty() {
            let (tx, rx) = oneshot::channel::<Result<(), DmxArrayError>>();

            self.to_array_tx
                .send(messages::ToArrayManagerMessage::RemoveGlobalValue(
                    value_name.to_owned(),
                    tx,
                ))
                .await
                .unwrap();

            if let Err(e) = rx.await.unwrap() {
                return Err(e).change_context_lazy(|| MqttMessageError::Context(format!("removing global value {value_name}")));
            }
        } else {
            let into_context = || MqttMessageError::Context(format!("adding global value {value_name}"));

            match serde_json::from_slice::<defs::ValueDefinition>(payload) {
                Ok(value_definition) => {
                    let (tx, rx) = oneshot::channel::<Result<(), DmxArrayError>>();

                    self.to_array_tx
                        .send(messages::ToArrayManagerMessage::AddGlobalValue(
                            value_name.clone(),
                            value_definition.value,
                            tx,
                        ))
                        .await
                        .unwrap();

                    if let Err(e) = rx.await.unwrap() {
                        return Err(e).change_context_lazy(into_context);
                    }
                }
                Err(e) => {
                    return Err(e).change_context_lazy(into_context)
                }
            }
        }

        Ok(())
    }

    async fn handle_effect_message(
        &self,
        effect_id: Arc<str>,
        payload: &Bytes,
    ) -> Result<(), MqttMessageError> {
        if payload.is_empty() {
            let (tx, rx) = oneshot::channel::<Result<(), DmxArrayError>>();

            self.to_array_tx
                .send(messages::ToArrayManagerMessage::RemoveEffect(
                    effect_id.clone(),
                    tx,
                ))
                .await
                .unwrap();

            if let Err(e) = rx.await.unwrap() {
                return Err(e).change_context_lazy(|| MqttMessageError::Context(format!("removing effect {effect_id}")));
            }
        } else {
            let into_context = || MqttMessageError::Context(format!("adding effect {effect_id}"));

            match serde_json::from_slice::<EffectNodeDefinition>(payload) {
                Ok(effect_definition) => {
                    let (tx, rx) = oneshot::channel::<Result<(), DmxArrayError>>();

                    self.to_array_tx
                        .send(messages::ToArrayManagerMessage::AddEffect(
                            effect_id.clone(),
                            effect_definition,
                            tx,
                        ))
                        .await
                        .unwrap();

                    if let Err(e) = rx.await.unwrap() {
                        return Err(e).change_context_lazy(into_context)
                    }
                }

                Err(e) => {
                    return Err(e).change_context_lazy(into_context)
                }
            }
        }
        Ok(())
    }

    async fn handle_command_message(
        &self,
        command: Arc<str>,
        payload: &Bytes,
    ) -> Result<(), MqttMessageError> {
        match command.as_ref() {
            "On" | "Off" | "Dim" => {

                let usage = command.parse::<EffectUsage>().unwrap();

                let command_parameters = serde_json::from_slice::<defs::OnOffCommandParameters>(
                    payload,
                ).change_context_lazy(|| MqttMessageError::Context(format!("parsing{command} command parameters")))?;

                let array_id = command_parameters.array_id.clone();
                let into_context = || MqttMessageError::Context(format!("{command} command on array {array_id}"));

                // If values were provided, set them as the array values
                if let Some(initial_values) = command_parameters.values {
                    let (tx, rx) = oneshot::channel::<Result<(), DmxArrayError>>();

                    self.to_array_tx
                        .send(messages::ToArrayManagerMessage::InitializeArrayValues(
                            command_parameters.array_id.clone(),
                            initial_values,
                            tx,
                        ))
                        .await
                        .unwrap();

                        let _ = rx.await.unwrap();
                }

                let (tx, rx) =
                    oneshot::channel::<Result<Box<dyn EffectNodeRuntime>, DmxArrayError>>();

                // Use the array ID as the effect ID
                let effect_id = command_parameters.array_id.clone();

                self.to_array_tx
                    .send(messages::ToArrayManagerMessage::GetEffectRuntime(
                        command_parameters.array_id,
                        usage,
                        command_parameters.effect_id,
                        command_parameters
                            .dimming_amount
                            .unwrap_or(DIMMING_AMOUNT_MAX),
                        tx,
                    ))
                    .await
                    .unwrap();

                let result = rx.await.unwrap();

                match result {
                    Err(e) => return Err(e).change_context_lazy(into_context),
                    Ok(effect_runtime_node) => {
                        let (tx, rx) = oneshot::channel::<Result<(), ArtnetError>>();

                        self.to_artnet_tx
                            .send(messages::ToArtnetManagerMessage::StartEffect(
                                effect_id,
                                effect_runtime_node,
                                tx,
                            ))
                            .await
                            .unwrap();

                        if let Err(e) = rx.await.unwrap() {
                            return Err(e).change_context_lazy(into_context);
                        }
                    }
                }
            }

            "Stop" => {
                let command_parameters = serde_json::from_slice::<defs::StopCommandParameters>(
                    payload,
                ).change_context_lazy(|| MqttMessageError::Context("parsing Stop command parameters".to_string()))?;

                let array_id = command_parameters.array_id.clone();
                let (tx, rx) = oneshot::channel::<Result<(), ArtnetError>>();

                self.to_artnet_tx
                    .send(messages::ToArtnetManagerMessage::StopEffect(
                        command_parameters.array_id,
                        tx,
                    ))
                    .await
                    .unwrap();
                if let Err(e) = rx.await.unwrap() {
                    return Err(e).change_context_lazy( || MqttMessageError::Context(format!("stopping effect on array {array_id}")))
                }
            }

            "Set" => {
                let command_parameters = serde_json::from_slice::<defs::SetChannelsParameters>(
                    payload,
                ).change_context_lazy(|| MqttMessageError::Context("parsing Set command parameters".to_string()))?;
                let universe_id = command_parameters.universe_id.clone();

                let (tx, rx) = oneshot::channel::<Result<(), ArtnetError>>();

                self.to_artnet_tx
                    .send(messages::ToArtnetManagerMessage::SetChannels(
                        command_parameters,
                        tx,
                    ))
                    .await
                    .unwrap();
                if let Err(e) = rx.await.unwrap() {
                    return Err(e).change_context_lazy(|| MqttMessageError::Context(format!("setting channels on universe {universe_id}")))
                }
            }
            _ => return Err(MqttMessageError::InvalidCommand(command.to_string()).into()),
        }

        Ok(())
    }
}
