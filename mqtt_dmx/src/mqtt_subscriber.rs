use error_stack::{Report, ResultExt};
use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;
use tracing::{error, info, info_span, Instrument};
use tokio::sync::{mpsc::Sender, oneshot};

use crate::{
    array_manager::DmxArrayError,
    artnet_manager::{ArtnetError, EffectNodeRuntime},
    defs::{self, EffectNodeDefinition, DIMMING_AMOUNT_MAX},
    defs::{EffectUsage, UniverseDefinition},
    messages,
    mqtt_pump::{Incoming, PumpEvent},
    persistence::Persistence,
    service::MqttError,
};

struct MqttSubscriber {
    to_artnet_tx: Sender<messages::ToArtnetManagerMessage>,
    to_array_tx: Sender<messages::ToArrayManagerMessage>,
    to_mqtt_publisher_tx: async_channel::Sender<messages::ToMqttPublisherMessage>,
    persistence: Arc<Persistence>,
    universes: HashMap<Arc<str>, defs::UniverseDefinition>,
    arrays: HashMap<Arc<str>, defs::DmxArray>,
    effects: HashMap<Arc<str>, defs::EffectNodeDefinition>,
    values: HashMap<Arc<str>, String>,
}

/// Handles the commands the pump forwards. It never polls: its waits — on the ArtNet and array
/// managers, and on the error queue to the publisher — hold back this task alone, while the pump
/// keeps draining rumqttc's request channel (no-hang §14.3).
pub async fn session(
    mut incoming: Incoming,
    to_artnet_tx: Sender<messages::ToArtnetManagerMessage>,
    to_array_tx: Sender<messages::ToArrayManagerMessage>,
    to_mqtt_publisher_tx: async_channel::Sender<messages::ToMqttPublisherMessage>,
    persistence: Arc<Persistence>,
) -> Result<(), Report<MqttError>> {
    info!("Starting MQTT subscriber session");
    let into_context = || MqttError::Context("In MQTT subscriber session".to_string());

    let mut mqtt_subscriber = MqttSubscriber {
        universes: persistence.load_universes(),
        arrays: persistence.load_arrays(),
        effects: persistence.load_effects(),
        values: persistence.load_values(),
        to_artnet_tx,
        to_array_tx,
        to_mqtt_publisher_tx,
        persistence,
    };

    loop {
        // WAIT: mqtt-pump-queue
        let publish = match incoming.recv().await {
            Some(PumpEvent::Publish(publish)) => publish,
            Some(PumpEvent::Ended(e)) => return Err(Report::new(MqttError::Context(format!("MQTT connection failed: {e}")))),
            None => return Err(Report::new(MqttError::Context("the MQTT pump stopped".to_string()))),
        };
        {
            let topic = String::from_utf8_lossy(&publish.topic).into_owned();
            let payload = publish.payload.clone();

            // Extract traceparent from MQTT 5 user properties
            let traceparent = publish.properties.as_ref().and_then(|p| {
                p.user_properties
                    .iter()
                    .find(|(k, _)| k == "traceparent")
                    .map(|(_, v)| v.clone())
            });

            let span = info_span!("mqtt_command", topic = %topic);
            if let Some(tp) = traceparent {
                tracing_init::traceparent::set_remote_parent(&span, &tp);
            }

            let result = async {
                if let Err(e) = mqtt_subscriber.handle_message(&topic, &payload).await {
                    error!(kind = "decode_error", topic = %topic, error = %e,
                           "MQTT message handling failed");
                    // WAIT: error-queue-send
                    mqtt_subscriber
                        .to_mqtt_publisher_tx
                        .send(messages::ToMqttPublisherMessage::Error(e.to_string()))
                        .await
                        .change_context_lazy(into_context)?;
                }
                Ok::<(), Report<MqttError>>(())
            }
            .instrument(span)
            .await;

            result?;
        }
    }
}

impl MqttSubscriber {
    async fn send_artnet(
        &self,
        msg: messages::ToArtnetManagerMessage,
    ) -> Result<(), Report<MqttError>> {
        // WAIT: artnet-queue
        self.to_artnet_tx
            .send(msg)
            .await
            .map_err(|_| Report::new(MqttError::ChannelClosed))
    }

    async fn send_array(
        &self,
        msg: messages::ToArrayManagerMessage,
    ) -> Result<(), Report<MqttError>> {
        // WAIT: array-queue
        self.to_array_tx
            .send(msg)
            .await
            .map_err(|_| Report::new(MqttError::ChannelClosed))
    }

    fn recv_reply<T>(
        result: Result<T, oneshot::error::RecvError>,
    ) -> Result<T, Report<MqttError>> {
        result.map_err(|_| Report::new(MqttError::ChannelClosed))
    }

    async fn handle_message(&mut self, topic: &str, payload: &Bytes) -> Result<(), Report<MqttError>> {
        let topic_parts: Vec<&str> = topic.split('/').collect();

        if topic_parts.len() < 2 {
            Err(MqttError::MissingSubtopic.into())
        } else {
            match topic_parts[1] {
                "Universe" => {
                    if topic_parts.len() != 3 {
                        Err(MqttError::MissingUniverseId(topic_parts[1].to_string()).into())
                    } else {
                        self.handle_universe_message(Arc::from(topic_parts[2]), payload)
                            .await
                    }
                }
                "Array" => {
                    if topic_parts.len() != 3 {
                        Err(MqttError::MissingArrayId(topic_parts[1].to_string()).into())
                    } else {
                        self.handle_array_message(Arc::from(topic_parts[2]), payload)
                            .await
                    }
                }
                "Command" => {
                    if topic_parts.len() != 3 {
                        Err(MqttError::MissingCommand.into())
                    } else {
                        self.handle_command_message(Arc::from(topic_parts[2]), payload)
                            .await
                    }
                }
                "Value" => {
                    if topic_parts.len() != 3 {
                        Err(MqttError::MissingCommand.into())
                    } else {
                        self.handle_value_message(Arc::from(topic_parts[2]), payload)
                            .await
                    }
                }
                "Effect" => {
                    if topic_parts.len() != 3 {
                        Err(MqttError::MissingCommand.into())
                    } else {
                        self.handle_effect_message(Arc::from(topic_parts[2]), payload)
                            .await
                    }
                }
                "Error" | "LastError" | "Active" | "Version" => Ok(()), // Ignore any message posted to Error subtopic since it is published by this service
                _ => Err(MqttError::InvalidSubtopic(topic_parts[1].to_string()).into()),
            }
        }
    }

    async fn handle_universe_message(
        &mut self,
        universe_id: Arc<str>,
        payload: &Bytes,
    ) -> Result<(), Report<MqttError>> {
        // If no payload is given, remove the universe
        if payload.is_empty() {
            let (tx_artnet_reply, rx_artnet_reply) = oneshot::channel::<Result<(), Report<ArtnetError>>>();

            self.send_artnet(messages::ToArtnetManagerMessage::RemoveUniverse(
                universe_id.clone(),
                tx_artnet_reply,
            ))
            .await?;

            // WAIT: artnet-reply
            if let Err(e) = Self::recv_reply(rx_artnet_reply.await)? {
                return Err(e)
                    .change_context_lazy(|| MqttError::Context(String::from("removing universe")));
            }

            self.universes.remove(&universe_id);
            self.persistence.save_universes(&self.universes);
        } else {
            match serde_json::from_slice::<UniverseDefinition>(payload) {
                Ok(definition) => {
                    let definition_clone = definition.clone();
                    let (tx_artnet_reply, rx_artnet_reply) =
                        oneshot::channel::<Result<(), Report<ArtnetError>>>();

                    self.send_artnet(messages::ToArtnetManagerMessage::AddUniverse(
                        universe_id.clone(),
                        definition,
                        tx_artnet_reply,
                    ))
                    .await?;

                    // WAIT: artnet-reply
                    if let Err(e) = Self::recv_reply(rx_artnet_reply.await)? {
                        return Err(e).change_context_lazy(|| {
                            MqttError::Context(format!("adding universe {universe_id}"))
                        });
                    }

                    self.universes.insert(universe_id, definition_clone);
                    self.persistence.save_universes(&self.universes);
                }
                Err(e) => {
                    return Err(MqttError::JsonParseError(
                        Arc::from("universe definition"),
                        universe_id.clone(),
                        e,
                    ))
                    .change_context_lazy(|| {
                        MqttError::Context(format!("parsing universe definition {universe_id}"))
                    });
                }
            }
        }

        Ok(())
    }

    async fn handle_array_message(
        &mut self,
        array_id: Arc<str>,
        payload: &Bytes,
    ) -> Result<(), Report<MqttError>> {
        // If no payload is given, remove the array
        if payload.is_empty() {
            let (tx, rx) = oneshot::channel::<Result<(), Report<DmxArrayError>>>();

            self.send_array(messages::ToArrayManagerMessage::RemoveArray(
                array_id.clone(),
                tx,
            ))
            .await?;

            // WAIT: array-reply
            if let Err(e) = Self::recv_reply(rx.await)? {
                return Err(e).change_context_lazy(|| {
                    MqttError::Context(format!("removing array {array_id}"))
                });
            }

            self.arrays.remove(&array_id);
            self.persistence.save_arrays(&self.arrays);
        } else {
            let into_context = || MqttError::Context(format!("adding array {array_id}"));

            match serde_json::from_slice::<defs::DmxArray>(payload) {
                Ok(definition) => {
                    let definition_clone = definition.clone();
                    let (tx, rx) = oneshot::channel::<Result<(), Report<DmxArrayError>>>();

                    self.send_array(messages::ToArrayManagerMessage::AddArray(
                        array_id.clone(),
                        Box::new(definition),
                        tx,
                    ))
                    .await?;

                    // WAIT: array-reply
                    if let Err(e) = Self::recv_reply(rx.await)? {
                        return Err(e).change_context_lazy(into_context);
                    }

                    self.arrays.insert(array_id, definition_clone);
                    self.persistence.save_arrays(&self.arrays);
                }
                Err(e) => return Err(e).change_context_lazy(into_context),
            }
        }

        Ok(())
    }

    async fn handle_value_message(
        &mut self,
        value_name: Arc<str>,
        payload: &Bytes,
    ) -> Result<(), Report<MqttError>> {
        if payload.is_empty() {
            let (tx, rx) = oneshot::channel::<Result<(), Report<DmxArrayError>>>();

            self.send_array(messages::ToArrayManagerMessage::RemoveGlobalValue(
                value_name.to_owned(),
                tx,
            ))
            .await?;

            // WAIT: array-reply
            if let Err(e) = Self::recv_reply(rx.await)? {
                return Err(e).change_context_lazy(|| {
                    MqttError::Context(format!("removing global value {value_name}"))
                });
            }

            self.values.remove(&value_name);
            self.persistence.save_values(&self.values);
        } else {
            let into_context = || MqttError::Context(format!("adding global value {value_name}"));

            match serde_json::from_slice::<defs::ValueDefinition>(payload) {
                Ok(value_definition) => {
                    let value = value_definition.value.clone();
                    let (tx, rx) = oneshot::channel::<Result<(), Report<DmxArrayError>>>();

                    self.send_array(messages::ToArrayManagerMessage::AddGlobalValue(
                        value_name.clone(),
                        value_definition.value,
                        tx,
                    ))
                    .await?;

                    // WAIT: array-reply
                    if let Err(e) = Self::recv_reply(rx.await)? {
                        return Err(e).change_context_lazy(into_context);
                    }

                    self.values.insert(value_name, value.to_string());
                    self.persistence.save_values(&self.values);
                }
                Err(e) => return Err(e).change_context_lazy(into_context),
            }
        }

        Ok(())
    }

    async fn handle_effect_message(
        &mut self,
        effect_id: Arc<str>,
        payload: &Bytes,
    ) -> Result<(), Report<MqttError>> {
        if payload.is_empty() {
            let (tx, rx) = oneshot::channel::<Result<(), Report<DmxArrayError>>>();

            self.send_array(messages::ToArrayManagerMessage::RemoveEffect(
                effect_id.clone(),
                tx,
            ))
            .await?;

            // WAIT: array-reply
            if let Err(e) = Self::recv_reply(rx.await)? {
                return Err(e).change_context_lazy(|| {
                    MqttError::Context(format!("removing effect {effect_id}"))
                });
            }

            self.effects.remove(&effect_id);
            self.persistence.save_effects(&self.effects);
        } else {
            let into_context = || MqttError::Context(format!("adding effect {effect_id}"));

            match serde_json::from_slice::<EffectNodeDefinition>(payload) {
                Ok(effect_definition) => {
                    let effect_definition_clone = effect_definition.clone();
                    let (tx, rx) = oneshot::channel::<Result<(), Report<DmxArrayError>>>();

                    self.send_array(messages::ToArrayManagerMessage::AddEffect(
                        effect_id.clone(),
                        effect_definition,
                        tx,
                    ))
                    .await?;

                    // WAIT: array-reply
                    if let Err(e) = Self::recv_reply(rx.await)? {
                        return Err(e).change_context_lazy(into_context);
                    }

                    self.effects.insert(effect_id, effect_definition_clone);
                    self.persistence.save_effects(&self.effects);
                }

                Err(e) => return Err(e).change_context_lazy(into_context),
            }
        }
        Ok(())
    }

    async fn handle_command_message(
        &mut self,
        command: Arc<str>,
        payload: &Bytes,
    ) -> Result<(), Report<MqttError>> {
        match command.as_ref() {
            "On" | "Off" | "Dim" => {
                let usage = command
                    .parse::<EffectUsage>()
                    .map_err(|e| Report::new(MqttError::InvalidCommand(e)))?;

                let command_parameters =
                    serde_json::from_slice::<defs::OnOffCommandParameters>(payload)
                        .change_context_lazy(|| {
                            MqttError::Context(format!("parsing {command} command parameters"))
                        })?;

                let array_id = command_parameters.array_id.clone();
                let into_context =
                    || MqttError::Context(format!("{command} command on array {array_id}"));

                // If values were provided, set them as the array values
                if let Some(initial_values) = command_parameters.values {
                    let (tx, rx) = oneshot::channel::<Result<(), Report<DmxArrayError>>>();

                    self.send_array(messages::ToArrayManagerMessage::InitializeArrayValues(
                        command_parameters.array_id.clone(),
                        initial_values,
                        tx,
                    ))
                    .await?;

                    // WAIT: array-reply
                    let _ = Self::recv_reply(rx.await)?;
                }

                let (tx, rx) =
                    oneshot::channel::<Result<Box<dyn EffectNodeRuntime>, Report<DmxArrayError>>>();

                // Use the array ID as the effect ID
                let effect_id = command_parameters.array_id.clone();

                self.send_array(messages::ToArrayManagerMessage::GetEffectRuntime(
                    command_parameters.array_id,
                    usage,
                    command_parameters.effect_id,
                    command_parameters
                        .dimming_amount
                        .unwrap_or(DIMMING_AMOUNT_MAX),
                    tx,
                ))
                .await?;

                // WAIT: array-reply
                let result = Self::recv_reply(rx.await)?;

                match result {
                    Err(e) => return Err(e).change_context_lazy(into_context),
                    Ok(effect_runtime_node) => {
                        let (tx, rx) = oneshot::channel::<Result<(), Report<ArtnetError>>>();

                        self.send_artnet(messages::ToArtnetManagerMessage::StartEffect(
                            effect_id,
                            effect_runtime_node,
                            tx,
                        ))
                        .await?;

                        // WAIT: artnet-reply
                        if let Err(e) = Self::recv_reply(rx.await)? {
                            return Err(e).change_context_lazy(into_context);
                        }
                    }
                }
            }

            "Stop" => {
                let command_parameters =
                    serde_json::from_slice::<defs::StopCommandParameters>(payload)
                        .change_context_lazy(|| {
                            MqttError::Context("parsing Stop command parameters".to_string())
                        })?;

                let array_id = command_parameters.array_id.clone();
                let (tx, rx) = oneshot::channel::<Result<(), Report<ArtnetError>>>();

                self.send_artnet(messages::ToArtnetManagerMessage::StopEffect(
                    command_parameters.array_id,
                    tx,
                ))
                .await?;

                // WAIT: artnet-reply
                if let Err(e) = Self::recv_reply(rx.await)? {
                    return Err(e).change_context_lazy(|| {
                        MqttError::Context(format!("stopping effect on array {array_id}"))
                    });
                }
            }

            "Set" => {
                let command_parameters =
                    serde_json::from_slice::<defs::SetChannelsParameters>(payload)
                        .change_context_lazy(|| {
                            MqttError::Context("parsing Set command parameters".to_string())
                        })?;
                let universe_id = command_parameters.universe_id.clone();

                let (tx, rx) = oneshot::channel::<Result<(), Report<ArtnetError>>>();

                self.send_artnet(messages::ToArtnetManagerMessage::SetChannels(
                    command_parameters,
                    tx,
                ))
                .await?;

                // WAIT: artnet-reply
                if let Err(e) = Self::recv_reply(rx.await)? {
                    return Err(e).change_context_lazy(|| {
                        MqttError::Context(format!("setting channels on universe {universe_id}"))
                    });
                }
            }
            _ => return Err(MqttError::InvalidCommand(command.to_string()).into()),
        }

        Ok(())
    }
}
