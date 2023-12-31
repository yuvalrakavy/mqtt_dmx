use log::info;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::{select, sync::mpsc::Receiver};
use tokio_util::sync::CancellationToken;
use error_stack::Result;

use super::error::DmxArrayError;
use crate::defs::{DmxArray, EffectNodeDefinition, SymbolTable};
use crate::messages::ToArrayManagerMessage;

#[derive(Debug)]
pub struct ArrayManager {
    pub(super) arrays: HashMap<Arc<str>, Box<DmxArray>>,
    pub(super) effects: HashMap<Arc<str>, EffectNodeDefinition>,
    pub(super) global_values: SymbolTable,
    pub(super) values: HashMap<Arc<str>, SymbolTable>,
    pub(super) default_on_effect: EffectNodeDefinition,
    pub(super) default_off_effect: EffectNodeDefinition,
    pub(super) default_dim_effect: EffectNodeDefinition,
}

impl ArrayManager {
    pub fn new() -> Self {
        let default_on_json = r#"
        {
            "type": "fade",
            "lights": "@all",
            "ticks": "`on_ticks=10`",
            "target": "`target=s(255);rgb(255,255,255);w(255,255,255)`"
        }"#;
        let default_off_json = r#"
        {
            "type": "fade",
            "lights": "@all",
            "ticks": "`off_ticks=10`",
            "target": "`target=s(0);rgb(0,0,0);w(0,0,0)`"
        }"#;
        let default_dim_json = r#"
        {
            "type": "fade",
            "lights": "@all",
            "ticks": "`dim_ticks=10`",
            "target": "`target=s(255);rgb(255,255,255);w(255,255,255)`"
        }"#;

        let default_on_effect =
            serde_json::from_str::<EffectNodeDefinition>(default_on_json).unwrap();
        let default_off_effect =
            serde_json::from_str::<EffectNodeDefinition>(default_off_json).unwrap();
        let default_dim_effect =
            serde_json::from_str::<EffectNodeDefinition>(default_dim_json).unwrap();

        Self {
            arrays: HashMap::new(),
            effects: HashMap::new(),
            global_values: HashMap::new(),
            values: HashMap::new(),
            default_on_effect,
            default_off_effect,
            default_dim_effect,
        }
    }

    pub fn add_array(
        &mut self,
        array_id: Arc<str>,
        array: Box<DmxArray>,
    ) -> Result<(), DmxArrayError> {
        self.verify_array(&array_id, &array)?;
        self.arrays.insert(array_id, array);
        Ok(())
    }

    pub fn remove_array(&mut self, name: Arc<str>) -> Result<(), DmxArrayError> {
        self.arrays.remove(&name);
        Ok(())
    }

    pub(super) fn get_array(&self, array_id: &str) -> Result<&DmxArray, DmxArrayError> {
        match self.arrays.get(array_id) {
            None => Err(DmxArrayError::ArrayNotFound(Arc::from(array_id)).into()),
            Some(array) => Ok(array),
        }
    }

    fn handle_message(&mut self, message: ToArrayManagerMessage) {
        match message {
            ToArrayManagerMessage::AddArray(array_id, array, reply_tx) => {
                reply_tx.send(self.add_array(array_id, array)).unwrap()
            }

            ToArrayManagerMessage::RemoveArray(array_id, reply_tx) => {
                reply_tx.send(self.remove_array(array_id)).unwrap()
            }

            ToArrayManagerMessage::AddGlobalValue(value_name, value, reply_tx) => {
                reply_tx.send(self.set_global_value(value_name, &value)).unwrap()
            }

            ToArrayManagerMessage::InitializeArrayValues(array_id, values, reply_tx) => {
                reply_tx.send(self.initialize_array_values(array_id, values)).unwrap()
            }

            ToArrayManagerMessage::RemoveGlobalValue(value_name, reply_tx) => {
                reply_tx.send(self.remove_global_value(&value_name)).unwrap()
            }

            ToArrayManagerMessage::AddEffect(effect_id, effect, reply_tx) => {
                reply_tx.send(self.add_effect(effect_id, effect)).unwrap()
            }

            ToArrayManagerMessage::RemoveEffect(effect_id, reply_tx) => {
                reply_tx.send(self.remove_effect(&effect_id)).unwrap()
            }

            ToArrayManagerMessage::GetEffectRuntime(
                array_id,
                effect_usage,
                effect_id,
                dimming_amount,
                reply_tx,
            ) => reply_tx
                .send(self.get_usage_effect_runtime(
                    &effect_usage,
                    &array_id,
                    effect_id.as_ref(),
                    dimming_amount,
                ))
                .unwrap(),
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
