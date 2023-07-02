use log::info;
use std::collections::HashMap;
use tokio::{select, sync::mpsc::Receiver};
use tokio_util::sync::CancellationToken;

use super::error::DmxArrayError;
use crate::defs::{DmxArray, EffectNodeDefinition};
use crate::messages::ToArrayManagerMessage;

#[derive(Debug)]
pub struct ArrayManager {
    pub(super) arrays: HashMap<String, DmxArray>,
    pub(super) effects: HashMap<String, EffectNodeDefinition>,
    pub(super) values: HashMap<String, String>,
}

impl ArrayManager {
    pub fn new() -> Self {
        Self {
            arrays: HashMap::new(),
            effects: HashMap::new(),
            values: HashMap::new(),
        }
    }

    pub fn add_array(
        &mut self,
        array_id: impl Into<String>,
        array: DmxArray,
    ) -> Result<(), DmxArrayError> {
        let array_id = array_id.into();
        self.verify_array(&array_id, &array)?;
        self.arrays.insert(array_id, array);
        Ok(())
    }

    pub fn remove_array(&mut self, name: String) -> Result<(), DmxArrayError> {
        self.arrays.remove(&name);
        Ok(())
    }

    pub(super) fn get_array(&self, array_id: &str) -> Result<&DmxArray, DmxArrayError> {
        self.arrays
            .get(array_id)
            .ok_or_else(|| DmxArrayError::ArrayNotFound(array_id.to_string()))
    }

    fn get_effect(
        &self,
        array_id: &str,
        effect_id: &str,
    ) -> Result<&EffectNodeDefinition, DmxArrayError> {
        let array = self.get_array(array_id)?;
        array
            .effects
            .get(effect_id)
            .or_else(|| self.effects.get(effect_id))
            .ok_or_else(|| {
                DmxArrayError::EffectNotFound(array_id.to_string(), effect_id.to_string())
            })
    }

    fn handle_message(&mut self, message: ToArrayManagerMessage) {
        match message {
            ToArrayManagerMessage::AddArray(array_id, array, reply_tx) => {
                reply_tx.send(self.add_array(array_id, array)).unwrap()
            }

            ToArrayManagerMessage::RemoveArray(array_id, reply_tx) => {
                reply_tx.send(self.remove_array(array_id)).unwrap()
            }

            ToArrayManagerMessage::GetLightChannels(array_id, lights_list, reply_tx) => reply_tx
                .send(self.get_array_light_channels(&array_id, &lights_list))
                .unwrap(),

            ToArrayManagerMessage::AddValues(values, reply_tx) => {
                reply_tx.send(self.add_values(values)).unwrap()
            }

            ToArrayManagerMessage::RemoveValues(reply_tx) => {
                reply_tx.send(self.remove_values()).unwrap()
            }
        }
    }

    pub async fn run(
        &mut self,
        cancel: CancellationToken,
        mut receiver: Receiver<ToArrayManagerMessage>,
    ) {
        loop {
            select! {
                _ = cancel.cancelled() => break,

                message = receiver.recv() => match message {
                    None => break,
                    Some(message) => self.handle_message(message),
                },
            }
        }

        info!("ArtnetManager stopped");
    }
}
