
use log::info;
use thiserror::Error;
use tokio::{select, sync::mpsc::Receiver};
use tokio_util::sync::CancellationToken;
use std::collections::HashMap;

use crate::messages::ToArrayManagerMessage;
use crate::defs::{DmxArray, Effect};
use crate::dmx::{ChannelDefinition, UniverseChannelDefinitions};


#[derive(Debug, Error)]
pub enum DmxArrayError {
    #[error("Array with id '{0}' not found")]
    ArrayNotFound(String),

    #[error("Array '{0}' Lights does not contain definition for {1}")]
    ArrayLightsNotFound(String, String),

    #[error("Array '{0}' Light '{1}' ({2}) contain circular reference to {3}")]
    ArrayLightsCircularReference(String, String, String, String),

    #[error("Array '{0}' Light '{1}' ({2}) is invalid channel definition (s:n, rgb:n or w:n)")]
    ArrayLightsInvalidChannelDefinition(String, String, String),

    #[error("Effect '{0}' not found in array '{1}' or in global effects list")]
    EffectNotFound(String, String),

    #[error("Value '{0}' not found in effect '{1}' or in array {2} values'")]
    EffectValueNotFound(String, String, String),

    #[error("Value '{0}' not found in array {1} values'")]
    ArrayValueNotFound(String, String),
}

pub struct ArrayManager {
    arrays: HashMap<String, DmxArray>,
    effects: HashMap<String, Effect>,
}

impl UniverseChannelDefinitions {
    fn new(universe_id: String) -> Self {
        Self {
            universe_id,
            channels: Vec::new(),
        }
    }

    fn add(&mut self, channel: ChannelDefinition) {
        self.channels.push(channel);
    }
}

impl ArrayManager {
    pub fn new() -> Self {
        Self {
            arrays: HashMap::new(),
            effects: HashMap::new(),
        }
    }

    pub fn add_array(&mut self, name: String, array: DmxArray) -> Result<(), DmxArrayError> {
        self.arrays.insert(name, array);
        Ok(())
    }

    pub fn remove_array(&mut self, name: String) -> Result<(), DmxArrayError> {
        self.arrays.remove(&name);
        Ok(())
    }

    fn get_array(&self, array_id: &str) -> Result<&DmxArray, DmxArrayError> {
        self.arrays.get(array_id).ok_or(DmxArrayError::ArrayNotFound(array_id.to_string()))
    }

    // Expand light channels string into a list of channel definitions
    //
    //  Syntax:
    //   <Entry1>,<Entry2>,<Entry3>,...
    //
    //  Entry:
    //   s:n | rgb:n | w:n | @array-light-entry-id | $universe-id
    //
    //  For example:
    //  {
    //   "universe": "0",
    //   "lights:" {
    //      "center": "rgb:1,rgb:4",
    //      "frame": "s:7",
    //      "spot": "$2, w:100",
    //      "all": "@center,@frame,@spot"
    //   }
    //  }
    //
    // The getting all will expand to the following:
    //  [
    //      UniverseChannelDefinitions { 
    //          universe_id: "0",
    //          [ ChannelDefinition { channel: 1, channel_type: ChannelType::Rgb }, ChannelDefinition { channel: 4, channel_type: ChannelType::Rgb }, ChannelDefinition { channel: 7, channel_type: ChannelType::Single } ]
    //      },
    //      UniverseChannelDefinitions {
    //          universe_id: "2",
    //          [ ChannelDefinition { channel: 100, channel_type: ChannelType::Tri_white } ]
    //      }
    //  ]
    //      
    fn get_array_light_channels(&self, array_id: &str, lights_id: &str) -> Result<Vec<UniverseChannelDefinitions>, DmxArrayError> {
        let array = self.get_array(array_id)?;
        let mut result = HashMap::<String, UniverseChannelDefinitions>::new();

        self.do_get_light_channels(array_id, lights_id, &mut result, 0)?;
        Ok(result.into_values().collect())
    }
    
    fn do_get_light_channels(&self, array_id: &str, lights_id: &str, result: &mut HashMap<String, UniverseChannelDefinitions>, nesting: u16) -> Result<(), DmxArrayError> {
        let array = self.get_array(array_id)?;
        let mut universe_id = array.universe_id.as_str();
        let light_channels = array.lights.get(lights_id).ok_or(DmxArrayError::ArrayLightsNotFound(array_id.to_string(), lights_id.to_string()))?;
        
        for entry in light_channels.split(',').map(|s| s.trim()) {
            if entry.starts_with('@') {
                if nesting > 10 {
                    return Err(DmxArrayError::ArrayLightsCircularReference(array_id.to_string(), lights_id.to_string(), light_channels.to_string(), entry.to_string()));
                }
                let nested_lighted_id = &entry[1..];

                self.do_get_light_channels(array_id, nested_lighted_id, result, nesting + 1)?;
            }
            else if entry.starts_with('$') {
                universe_id = &entry[1..];
            }
            else {
                let channel = entry.parse::<ChannelDefinition>().
                    map_err(|_| DmxArrayError::ArrayLightsInvalidChannelDefinition(array_id.to_string(), lights_id.to_string(), entry.to_string()))?;
                let universe_channels = result.entry(universe_id.to_string()).or_insert(UniverseChannelDefinitions::new(universe_id.to_string()));
                universe_channels.add(channel); 
            }
        }

        Ok(())
    }

    fn get_effect(&self, array_id: &str, effect_id: &str) -> Result<&Effect, DmxArrayError> {
        let array = self.get_array(array_id)?;
        array.effects.get(effect_id).or_else(|| self.effects.get(effect_id)).ok_or(DmxArrayError::EffectNotFound(array_id.to_string(), effect_id.to_string()))
    }

    fn get_value(&self, values: Option<&HashMap<String, String>>, array_id: &str, effect_id: Option<&str>, value_name: &str) -> Result<Option<String>, DmxArrayError> {
        if let Some(values) = values {
            if let Some(value) = values.get(value_name) {
                return Ok(Some(value.to_string()));
            }
        }

        let array = self.get_array(array_id)?;

        if let Some(effect_id) = effect_id {
            let effect = self.get_effect(array_id, effect_id)?;

            if let Some(value) = effect.values.get(value_name) {
                return Ok(Some(value.to_string()));
            }
        }

        Ok(array.values.get(value_name).map(|s| s.to_string()))
    }


    fn handle_message(&mut self, message: ToArrayManagerMessage) {
         match message {
            ToArrayManagerMessage::AddArray(array_id, array, reply_tx) =>
                reply_tx.send(self.add_array(array_id, array)).unwrap(),

            ToArrayManagerMessage::RemoveArray(array_id, reply_tx) =>
                reply_tx.send(self.remove_array(array_id)).unwrap(),

            ToArrayManagerMessage::GetLightChannels(array_id, light_id, reply_tx) =>
                reply_tx.send(self.get_array_light_channels(&array_id, &light_id)).unwrap()
        }
    }

    pub async fn run(&mut self, cancel: CancellationToken, mut receiver: Receiver<ToArrayManagerMessage>) {
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

#[cfg(test)]
mod test_array_manager {
    use crate::dmx::ChannelType;

    use super::*;

    fn get_array() -> DmxArray {
        let lights = HashMap::from([
           ("center".to_string(), "rgb:1,rgb:4".to_string()),
           ("frame".to_string(), "s:7".to_string()),
           ("spot".to_string(), "$2, w:100".to_string()),
           ("all".to_string(), "@center,@spot,@frame".to_string()),
           ("Invalid".to_string(), "x:4".to_string()),
           ("circular".to_string(), "@circular".to_string())
        ]);

        DmxArray {
            universe_id: "0".to_string(),
            description: "Test array".to_string(),
            lights,
            effects: HashMap::new(),
            values: HashMap::new(),
            presets: Vec::new(),
        }
    }

    #[test]
    fn test_get_array_light_channels() {
        let mut array_manager = ArrayManager::new();
        let array = get_array();

        array_manager.add_array("test".to_string(), array).unwrap();

        let result = array_manager.get_array_light_channels("test", "all").unwrap();
        let u0 = if result[0].universe_id == "0" { 0 } else { 1 };
        let u1 = if result[0].universe_id == "2" { 0 } else { 1 };

        assert_eq!(result.len(), 2);
        assert_eq!(result[u0].universe_id, "0");
        assert_eq!(result[u0].channels.len(), 3);
        assert_eq!(result[u0].channels[0], ChannelDefinition { channel: 1, channel_type: ChannelType::Rgb} );
        assert_eq!(result[u0].channels[1], ChannelDefinition { channel: 4, channel_type: ChannelType::Rgb} );
        assert_eq!(result[u0].channels[2], ChannelDefinition { channel: 7, channel_type: ChannelType::Single} );
        assert_eq!(result[u1].universe_id, "2");
        assert_eq!(result[u1].channels.len(), 1);
        assert_eq!(result[u1].channels[0], ChannelDefinition { channel: 100, channel_type: ChannelType::TriWhite} );

        // Test error handling
        let result = array_manager.get_array_light_channels("test", "Invalid");
        assert!(result.is_err());

        if let Err(e) = result {
            let t = e.to_string();
            assert_eq!(t, "Array 'test' Light 'Invalid' (x:4) is invalid channel definition (s:n, rgb:n or w:n)");
        }

        let result = array_manager.get_array_light_channels("test", "circular");
        assert!(result.is_err());

        if let Err(e) = result {
            let t = e.to_string();
            assert_eq!(t, "Array 'test' Light 'circular' (@circular) contain circular reference to @circular");
        }
    }
}