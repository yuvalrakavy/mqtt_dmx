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
    pub(super) default_on_effect: EffectNodeDefinition,
    pub(super) default_off_effect: EffectNodeDefinition,
}

impl ArrayManager {
    pub fn new() -> Self {
        let default_on_json = r#"
        {
            "type": "fade",
            "lights": "@all",
            "ticks": "`default_ticks=10`",
            "target": "`default_target=s(255);rgb(255,255,255);w(255,255,255)`"
        }"#;
        let default_off_json = r#"
        {
            "type": "fade",
            "lights": "@all",
            "ticks": "`default_ticks=10`",
            "target": "`default_target=s(0);rgb(0,0,0);w(0,0,0)`"
        }"#;

        let default_on_effect =
            serde_json::from_str::<EffectNodeDefinition>(default_on_json).unwrap();
        let default_off_effect =
            serde_json::from_str::<EffectNodeDefinition>(default_off_json).unwrap();

        Self {
            arrays: HashMap::new(),
            effects: HashMap::new(),
            values: HashMap::new(),
            default_on_effect,
            default_off_effect,
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

    fn handle_message(&mut self, message: ToArrayManagerMessage) {
        match message {
            ToArrayManagerMessage::AddArray(array_id, array, reply_tx) => {
                reply_tx.send(self.add_array(array_id, array)).unwrap()
            }

            ToArrayManagerMessage::RemoveArray(array_id, reply_tx) => {
                reply_tx.send(self.remove_array(array_id)).unwrap()
            }

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
