
use log::info;
use thiserror::Error;
use tokio::{select, sync::mpsc::Receiver};
use tokio_util::sync::CancellationToken;
use std::collections::HashMap;
use std::fmt::Display;

use crate::messages::ToArrayManagerMessage;
use crate::defs::{DmxArray, EffectNodeDefinition};
#[cfg(test)]
use crate::defs::DmxArrayPreset;
use crate::dmx::{ChannelDefinition, UniverseChannelDefinitions, ChannelType};


#[derive(Debug, Error)]
pub enum DmxArrayError {
    #[error("Array with id '{0}' not found")]
    ArrayNotFound(String),

    #[error("Array '{0}' Lights {1} does not contain definition for {2}")]
    ArrayLightsNotFound(String, String, String),

    #[error("Array '{0}' Light '{1}' ({2}) contain circular reference to {3}")]
    ArrayLightsCircularReference(String, String, String, String),

    #[error("Array '{0}' Light '{1}' ({2}) is invalid channel definition (s:n, rgb:n or w:n)")]
    ArrayLightsInvalidChannelDefinition(String, String, String),

    #[error("Effect '{0}' not found in array '{1}' or in global effects list")]
    EffectNotFound(String, String),

    #[error("Value '{0}' not found in effect '{1}' or in array {2} values'")]
    EffectValueNotFound(String, String, String),

    #[error("Array '{0}' has no preset# {1} defined")]
    ArrayPresetNotFound(String, usize),

    #[error("Array '{0}' preset# {1} '{2}' has no value for {3}")]
    ArrayPresetValueNotFound(String, usize, String, String),

    #[error("Array '{0}' '{1}' has no value for {2}")]
    ArrayValueNotFound(String, String, String),

    #[error("Array '{0}' '{1}' has unterminated `value` expression")]
    ValueExpressionNotTerminated(String, String),

    #[error("Array '{0}' has no presets and no default 'on' or 'off' effects are defined")]
    ArrayNoDefaultEffects(String),

    #[error("Array '{0} has no lights group named 'all', this light group is mandatory")]
    ArrayNoAllLightsGroup(String),

    #[error("Array '{0}' preset {1} '{2}' effect is '{3}' which is not defined")]
    ArrayPresetEffectNotFound(String, usize, &'static str, String),

    #[error("Array '{0}' preset {1} {2} effect use default on effect which is not defined")]
    ArrayPresetDefaultEffectNotFound(String, usize, &'static str),

    #[error("Array '{0}' in universe '{1}': channel {2} was defined as {3} and is redefined as {4} in group @{5}")]
    ArrayLightChannelUsageMismatch(String, String, u16, ChannelUsage, ChannelUsage, String),

    #[error("Array '{0}' in universe '{1}': channel {2} is defined as {3} in group @{4} but is not included in @all group")]
    ArrayLightChannelNotInAllGroup(String, String, u16, ChannelUsage, String),
}

#[derive(Debug)]
pub struct ArrayManager {
    arrays: HashMap<String, DmxArray>,
    effects: HashMap<String, EffectNodeDefinition>,
    values: HashMap<String, String>,
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

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum ChannelUsage {
    S,
    R,
    G,
    B,
    W1,
    W2,
    W3,
}

impl Display for ChannelUsage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            ChannelUsage::S => write!(f, "single light channel"),
            ChannelUsage::R => write!(f, "light red component"),
            ChannelUsage::G => write!(f, "light green component"),
            ChannelUsage::B => write!(f, "light blue component"),
            ChannelUsage::W1 => write!(f, "light w1 component"),
            ChannelUsage::W2 => write!(f, "light w2 component"),
            ChannelUsage::W3 => write!(f, "light w3 component"),
        }
    }
}

#[derive(Debug)]
pub struct Scope<'a> {
    array_manager: &'a ArrayManager,
    pub array_id: String,
    pub preset_number: Option<usize>,
    pub values: Option<HashMap<String, String>>,
}

impl Scope<'_> {
    pub fn new(array_manager: &ArrayManager, array_id: impl Into<String>, preset_number: Option<usize>, values: Option<HashMap<String, String>>) -> Result<Scope, DmxArrayError> {
        let array_id = array_id.into();
        let array = array_manager.arrays.get(&array_id);

        if array.is_none() {
            return Err(DmxArrayError::ArrayNotFound(array_id));
        }

        let array = array.unwrap();

        if let Some(preset_number) = preset_number {
            if preset_number >= array.presets.len() {
                return Err(DmxArrayError::ArrayPresetNotFound(array_id, preset_number));
            }
        }

        Ok(Scope {
            array_manager,
            array_id,
            preset_number,
            values,
        })
    }

    pub fn get_light_channels(&self, lights_list: &str) -> Result<Vec<UniverseChannelDefinitions>, DmxArrayError> {
        self.array_manager.get_array_light_channels(&self.array_id, lights_list)
    }

    pub fn expand_values(&self, unexpanded_value: &str) -> Result<String, DmxArrayError> {
        self.array_manager.expand_values(self, unexpanded_value)
    }

    pub fn get_dimmer_level(&self) -> u16 {
        self.array_manager.get_array_dimmer_level(&self.array_id).unwrap()
    }
}

struct ExpansionStack {
    stack: Vec<String>,
}

impl ExpansionStack {
    fn new() -> Self {
        Self {
            stack: Vec::new(),
        }
    }

    fn push(&mut self, id: String) {
        self.stack.push(id);
    }

    fn pop(&mut self) {
        self.stack.pop().unwrap();
    }

    fn len(&self) -> usize {
        self.stack.len()
    }
}

impl Display for ExpansionStack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut first = true;
        for id in &self.stack {
            if first {
                first = false;
            } else {
                write!(f, " -> ")?;
            }
            write!(f, "{}", id)?;
        }
        Ok(())
    }
}

impl ArrayManager {
    pub fn new() -> Self {
        Self {
            arrays: HashMap::new(),
            effects: HashMap::new(),
            values: HashMap::new(),
        }
    }

    pub fn add_array(&mut self, array_id: impl Into<String>, array: DmxArray) -> Result<(), DmxArrayError> {
        let array_id = array_id.into();
        self.verify_array(&array_id, &array)?;
        self.arrays.insert(array_id, array);
        Ok(())
    }

    fn verify_array(&self, array_id: &str, array: &DmxArray) -> Result<(), DmxArrayError> {
        self.verify_array_effects(array_id, array)?;
        Self::verify_array_lights(array_id, array)?;
        Ok(())
    }

    fn verify_array_effects(&self, array_id: &str, array: &DmxArray) -> Result<(), DmxArrayError> {
        let has_effect = |effect_name: &str| {
            array.effects.get(effect_name).is_some() || self.effects.get(effect_name).is_some()
        };

        let has_on_effect = array.effects.get("on").or_else(|| self.effects.get("on")).is_some();
        let has_off_effect = array.effects.get("off").or_else(|| self.effects.get("off")).is_some();

        if array.presets.is_empty() && !has_on_effect && !has_off_effect {
            return Err(DmxArrayError::ArrayNoDefaultEffects(array_id.to_string()));
        }

        for (preset_number, preset) in array.presets.iter().enumerate() {
            if let Some(ref effect_name) = preset.on {
                if !has_effect(effect_name) {
                    return Err(DmxArrayError::ArrayPresetEffectNotFound(array_id.to_string(), preset_number, "on", effect_name.to_string()));
                }
            }
            else if !has_on_effect {
                return Err(DmxArrayError::ArrayPresetDefaultEffectNotFound(array_id.to_string(), preset_number, "on"));
            }

            if let Some(ref effect_name) = preset.off {
                if !has_effect(effect_name) {
                    return Err(DmxArrayError::ArrayPresetEffectNotFound(array_id.to_string(), preset_number, "off", effect_name.to_string()));
                }
            }
            else if !has_on_effect {
                return Err(DmxArrayError::ArrayPresetDefaultEffectNotFound(array_id.to_string(), preset_number, "off"));
            }
        }

        Ok(())
    }

    fn verify_array_lights(array_id: &str, array: &DmxArray) -> Result<(), DmxArrayError> {
        let add_light_usage = |group_name: &str, channel_usage: &mut HashMap<String, HashMap<u16, ChannelUsage>>, must_exist: bool, lights: Vec<UniverseChannelDefinitions>|
          -> Result<(), DmxArrayError> {
            for universe_channel_definition in lights.iter() {
                let universe_usage = channel_usage.entry(universe_channel_definition.universe_id.clone()).or_insert_with(HashMap::new);
                let mut add_channel_usage = |channel: u16, usage: ChannelUsage| -> Result<(), DmxArrayError> {
                    if let Some(existing_usage) = universe_usage.get(&channel) {
                        if *existing_usage != usage {
                            return Err(DmxArrayError::ArrayLightChannelUsageMismatch(array_id.to_string(), universe_channel_definition.universe_id.clone(), channel,
                             *existing_usage, usage, group_name.to_string()));
                        }
                    }
                    else if must_exist {
                        return Err(DmxArrayError::ArrayLightChannelNotInAllGroup(array_id.to_string(), universe_channel_definition.universe_id.clone(), channel, usage, group_name.to_string()))
                    }
                    else {
                        universe_usage.insert(channel, usage);
                    }

                    Ok(())
                };

                for channel_definition in universe_channel_definition.channels.iter() {
                    match channel_definition.channel_type {
                        ChannelType::Single => add_channel_usage(channel_definition.channel, ChannelUsage::S)?,
                        ChannelType::Rgb => {
                            add_channel_usage(channel_definition.channel, ChannelUsage::R)?;
                            add_channel_usage(channel_definition.channel + 1, ChannelUsage::G)?;
                            add_channel_usage(channel_definition.channel + 2, ChannelUsage::B)?;
                        },
                        ChannelType::TriWhite => {
                            add_channel_usage(channel_definition.channel, ChannelUsage::W1)?;
                            add_channel_usage(channel_definition.channel + 1, ChannelUsage::W2)?;
                            add_channel_usage(channel_definition.channel + 2, ChannelUsage::W3)?;
                        },
                    }
                }
            }
            Ok(())
        };

        let mut channel_usage: HashMap<String, HashMap<u16, ChannelUsage>> = HashMap::new();
        let all_lights = Self::static_get_array_light_channels(array_id, array, "@all")?;

        add_light_usage("@all", &mut channel_usage, false, all_lights)?;

        for (light_group_name, lights_list) in array.lights.iter() {
            let lights = Self::static_get_array_light_channels(array_id, array, lights_list)?;
            add_light_usage(light_group_name, &mut channel_usage, true, lights)?;
        }

        Ok(())
    }

    pub fn remove_array(&mut self, name: String) -> Result<(), DmxArrayError> {
        self.arrays.remove(&name);
        Ok(())
    }

    fn add_values(&mut self, values: HashMap<String, String>) -> Result<(), DmxArrayError> {
        self.values = values;
        Ok(())
    }

    fn remove_values(&mut self) -> Result<(), DmxArrayError>{
        self.values.clear();
        Ok(())
    }

    fn get_array(&self, array_id: &str) -> Result<&DmxArray, DmxArrayError> {
        self.arrays.get(array_id).ok_or_else(|| DmxArrayError::ArrayNotFound(array_id.to_string()))
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
    fn static_do_get_array_light_channels(array_id: &str, array: &DmxArray, lights_list: &str, result: &mut HashMap<String, UniverseChannelDefinitions>, stack: &mut ExpansionStack) -> Result<(), DmxArrayError> {
        let mut universe_id = array.universe_id.as_str();
        
        for entry in lights_list.split(',').map(|s| s.trim()) {
            if let Some(nested_lighted_id) = entry.strip_prefix('@') {
                let nested_lights_list = array.lights.get(nested_lighted_id).ok_or_else(|| DmxArrayError::ArrayLightsNotFound(array_id.to_string(), stack.to_string(), nested_lighted_id.to_string()))?;


                stack.push(nested_lights_list.to_string());

                if stack.len() > 5 {
                    // Array '{0}' Light '{1}' ({2}) contain circular reference to {3}
                    return Err(DmxArrayError::ArrayLightsCircularReference(array_id.to_string(), stack.to_string(), nested_lights_list.to_string(), entry.to_string()));
                }

                Self::static_do_get_array_light_channels(array_id, array, nested_lights_list, result, stack)?;
                stack.pop();
            }
            else if let Some(entry) = entry.strip_prefix('$') {
                universe_id = entry;
            }
            else {
                let channel = entry.parse::<ChannelDefinition>().
                    map_err(|_| DmxArrayError::ArrayLightsInvalidChannelDefinition(array_id.to_string(), stack.to_string(), entry.to_string()))?;
                let universe_channels = result.entry(universe_id.to_string()).or_insert_with(|| UniverseChannelDefinitions::new(universe_id.to_string()));
                universe_channels.add(channel); 
            }
        }

        Ok(())
    }

    fn static_get_array_light_channels(array_id: &str, array: &DmxArray, lights_list: &str) -> Result<Vec<UniverseChannelDefinitions>, DmxArrayError> {
        let mut result = HashMap::<String, UniverseChannelDefinitions>::new();
        let mut stack = ExpansionStack::new();

        stack.push(lights_list.to_string());
        Self::static_do_get_array_light_channels(array_id, array, lights_list, &mut result, &mut stack)?;
        stack.pop();

        Ok(result.into_values().collect())
    }

    pub fn get_array_light_channels(&self, array_id: &str, lights_list: &str) -> Result<Vec<UniverseChannelDefinitions>, DmxArrayError> {
        let array = self.get_array(array_id)?;
        Self::static_get_array_light_channels(array_id, array, lights_list)
    }

    pub fn get_array_all_lights(&self, array_id: &str) -> Result<Vec<UniverseChannelDefinitions>, DmxArrayError> {
        self.get_array_light_channels(array_id, "@all")
    }

    pub fn get_array_dimmed_lights(&self, array_id: &str) -> Result<Vec<UniverseChannelDefinitions>, DmxArrayError> {
        let array = self.get_array(array_id)?;

        if array.lights.contains_key("dimmed") {
            self.get_array_light_channels(array_id, "@dimmed")
        }
        else {
            self.get_array_all_lights(array_id)
        }
    }

    pub fn get_array_dimmer_level(&self, array_id: &str) -> Result<u16, DmxArrayError> {
        let array = self.get_array(array_id)?;
        Ok(array.dimmer_level)
    }

    fn get_effect(&self, array_id: &str, effect_id: &str) -> Result<&EffectNodeDefinition, DmxArrayError> {
        let array = self.get_array(array_id)?;
        array.effects.get(effect_id).or_else(|| self.effects.get(effect_id)).ok_or_else(|| DmxArrayError::EffectNotFound(array_id.to_string(), effect_id.to_string()))
    }

    fn get_value(&self, scope: &Scope, value_name: &str) -> Result<Option<String>, DmxArrayError> {
        if let Some(values) = &scope.values {
            if let Some(value) = values.get(value_name) {
                return Ok(Some(value.to_string()));
            }
        }

        let array = self.get_array(&scope.array_id)?;

        if let Some(preset_number) = scope.preset_number {
            if let Some(preset) = array.presets.get(preset_number) {
                if let Some(value) = preset.values.get(value_name) {
                    return Ok(Some(value.to_string()));
                }
            }
            else {
                return Err(DmxArrayError::ArrayPresetNotFound(scope.array_id.clone(), preset_number));
            }
        }

        if let Some(value) = array.values.get(value_name) {
            return Ok(Some(value.to_string()));
        }

        Ok(self.values.get(value_name).map(|s| s.to_string()))
    }

    fn expand_values(&self, scope: &Scope, unexpanded_value: &str) -> Result<String, DmxArrayError> {
        let mut value = unexpanded_value;
        let mut result = String::new();
        let index = 0;

        while let Some(value_name_start_index) = value[index..].find('`') {
            result.push_str(&value[..value_name_start_index]);
            value = &value[value_name_start_index + 1..];

            if let Some(value_name_end_index) = value.find('`') {
                let value_name_expression = &value[..value_name_end_index];
                let (value_name, default_value) = if let Some(default_value_index) = value_name_expression.find('=') {
                    (&value_name_expression[..default_value_index], Some(&value_name_expression[default_value_index + 1..]))
                }
                else {
                    (value_name_expression, None)
                };

                let expanded_value = self.get_value(scope, value_name)?;

                if let Some(expanded_value) = expanded_value {
                    result.push_str(&expanded_value);
                }
                else if let Some(default_value) = default_value {
                    result.push_str(default_value);
                }
                else {
                    return Err(
                        if let Some(preset_number) = scope.preset_number {
                            DmxArrayError::ArrayPresetValueNotFound(scope.array_id.to_string(), preset_number, unexpanded_value.to_string(), value_name.to_string())
                        }
                        else {
                            DmxArrayError::ArrayValueNotFound(scope.array_id.to_string(), unexpanded_value.to_string(), value_name.to_string())
                        }
                    );
                }

                value = &value[value_name_end_index + 1..];
            }
            else {
                return Err(DmxArrayError::ValueExpressionNotTerminated(scope.array_id.clone(), unexpanded_value.to_string()));
            }
        }

        result.push_str(value);

        Ok(result)
    }

    fn handle_message(&mut self, message: ToArrayManagerMessage) {
         match message {
            ToArrayManagerMessage::AddArray(array_id, array, reply_tx) =>
                reply_tx.send(self.add_array(array_id, array)).unwrap(),

            ToArrayManagerMessage::RemoveArray(array_id, reply_tx) =>
                reply_tx.send(self.remove_array(array_id)).unwrap(),

            ToArrayManagerMessage::GetLightChannels(array_id, lights_list, reply_tx) =>
                reply_tx.send(self.get_array_light_channels(&array_id, &lights_list)).unwrap(),

            ToArrayManagerMessage::AddValues(values, reply_tx) =>
                reply_tx.send(self.add_values(values)).unwrap(),

            ToArrayManagerMessage::RemoveValues(reply_tx) =>
                reply_tx.send(self.remove_values()).unwrap(),        }
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

    #[test]
    fn test_verify_array() {
        let array_json = r#"
            {
                "universe_id": "0",
                "description": "Test array",
                "lights": {
                    "all": "rgb:0"
                },
                "dimmer_level": 1000,
                "effects": {
                    "on": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(255); rgb(255,255,255); w(255)"
                    },
                    "off": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(0); rgb(0,0,0); w(0)"
                    }
                },
                "values": {
                },
                "presets": [
                    {
                        "description": "preset1",
                        "values": {
                        }
                    }
                ]
            }"#;

        let mut array_manager = ArrayManager::new();
        let array = serde_json::from_str::<DmxArray>(array_json).unwrap();

        array_manager.add_array("test", array).unwrap();

        let array_json = r#"
            {
                "universe_id": "0",
                "description": "Test array",
                "lights": {
                    "all": "rgb:10"
                },
                "dimmer_level": 1000,
                "effects": {
                    "on": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(255); rgb(255,255,255); w(255)"
                    },
                    "off": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(0); rgb(0,0,0); w(0)"
                    },
                    "custom": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(128); rgb(0,0,0); w(0)"
                    }
                },
                "values": {
                },
                "presets": [
                    {
                        "description": "preset1",
                        "on": "custom",
                        "values": {
                        }
                    }
                ]
            }"#;

        let array = serde_json::from_str::<DmxArray>(array_json).unwrap();

        array_manager.add_array("test2", array).unwrap();

        let array_json = r#"
            {
                "universe_id": "0",
                "description": "Test array",
                "lights": {
                    "all": "rgb:10"
                },
                "dimmer_level": 1000,
                "effects": {
                    "on": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(255); rgb(255,255,255); w(255)"
                    },
                    "off": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(0); rgb(0,0,0); w(0)"
                    }
                },
                "values": {
                },
                "presets": [
                    {
                        "description": "preset1",
                        "on": "custom",
                        "values": {
                        }
                    }
                ]
            }"#;

        let array = serde_json::from_str::<DmxArray>(array_json).unwrap();

        if let Err(e) = array_manager.add_array("test3", array) {
            let t = e.to_string();
            assert_eq!(t, "Array 'test3' preset 0 'on' effect is 'custom' which is not defined");
        } else {
            panic!("Expected error");
        }

        let array_json = r#"
            {
                "universe_id": "0",
                "description": "Test array",
                "lights": {
                    "all": "@center,@spot,@frame"
                },
                "dimmer_level": 1000,
                "effects": {
                    "on": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(255); rgb(255,255,255); w(255)"
                    },
                    "off": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(0); rgb(0,0,0); w(0)"
                    }
                },
                "values": {
                },
                "presets": [
                    {
                        "description": "preset1",
                        "off": "custom",
                        "values": {
                        }
                    }
                ]
            }"#;

        let array = serde_json::from_str::<DmxArray>(array_json).unwrap();

        if let Err(e) = array_manager.add_array("test3", array) {
            let t = e.to_string();
            assert_eq!(t, "Array 'test3' preset 0 'off' effect is 'custom' which is not defined");
        } else {
            panic!("Expected error");
        }

        let array_json = r#"
            {
                "universe_id": "0",
                "description": "Test array",
                "lights": {
                    "center": "rgb:10",
                    "spot": "s:20",
                    "frame": "w:30",
                    "all": "@center,@spot,@frame"
                },
                "dimmer_level": 1000,
                "effects": {
                    "on": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(255); rgb(255,255,255); w(255)"
                    },
                    "off": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(0); rgb(0,0,0); w(0)"
                    }
                },
                "values": {
                },
                "presets": [
                ]
            }"#;

        let array = serde_json::from_str::<DmxArray>(array_json).unwrap();

        array_manager.add_array("test2", array).unwrap();

        let array_json = r#"
            {
                "universe_id": "0",
                "description": "Test array",
                "lights": {
                    "center": "rgb:10",
                    "spot": "s:20",
                    "frame": "w:30",
                    "outside": "rgb:40",
                    "all": "@center,@spot,@frame"
                },
                "dimmer_level": 1000,
                "effects": {
                    "on": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(255); rgb(255,255,255); w(255)"
                    },
                    "off": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(0); rgb(0,0,0); w(0)"
                    }
                },
                "values": {
                },
                "presets": [
                ]
            }"#;

        let array = serde_json::from_str::<DmxArray>(array_json).unwrap();

        if let Err(e) = array_manager.add_array("test2", array) {
            let t = e.to_string();
            assert_eq!(t, "Array 'test2' in universe '0': channel 40 is defined as light red component in group @outside but is not included in @all group");
        }

        let array_json = r#"
            {
                "universe_id": "0",
                "description": "Test array",
                "lights": {
                    "all": "rgb:0,rgb:1"
                },
                "dimmer_level": 1000,
                "effects": {
                    "on": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(255); rgb(255,255,255); w(255)"
                    },
                    "off": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(0); rgb(0,0,0); w(0)"
                    }
                },
                "values": {
                },
                "presets": [
                ]
            }"#;

        let array = serde_json::from_str::<DmxArray>(array_json).unwrap();

        if let Err(e) = array_manager.add_array("test2", array) {
            let t = e.to_string();
            assert_eq!(t, "Array 'test2' in universe '0': channel 1 was defined as light green component and is redefined as light red component in group @@all");
        }

        let array_json = r#"
            {
                "universe_id": "0",
                "description": "Test array",
                "lights": {
                    "all": "rgb:0,x:5"
                },
                "dimmer_level": 1000,
                "effects": {
                    "on": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(255); rgb(255,255,255); w(255)"
                    },
                    "off": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(0); rgb(0,0,0); w(0)"
                    }
                },
                "values": {
                },
                "presets": [
                ]
            }"#;

        let array = serde_json::from_str::<DmxArray>(array_json).unwrap();

        if let Err(e) = array_manager.add_array("test2", array) {
            let t = e.to_string();
            assert_eq!(t, "Array 'test2' Light '@all -> rgb:0,x:5' (x:5) is invalid channel definition (s:n, rgb:n or w:n)");
        }

        let array_json = r#"
            {
                "universe_id": "0",
                "description": "Test array",
                "lights": {
                    "all": "rgb:0,@loop",
                    "loop": "rgb:3,@circle",
                    "circle": "@loop"
                },
                "dimmer_level": 1000,
                "effects": {
                    "on": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(255); rgb(255,255,255); w(255)"
                    },
                    "off": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(0); rgb(0,0,0); w(0)"
                    }
                },
                "values": {
                },
                "presets": [
                ]
            }"#;

        let array = serde_json::from_str::<DmxArray>(array_json).unwrap();

        if let Err(e) = array_manager.add_array("test2", array) {
            let t = e.to_string();
            assert_eq!(t, "Array 'test2' Light '@all -> rgb:0,@loop -> rgb:3,@circle -> @loop -> rgb:3,@circle -> @loop' (@loop) contain circular reference to @circle");
        }


    }

    #[test]
    fn test_get_array_light_channels() {
        let mut array_manager = ArrayManager::new();
        let array_json = r#"
            {
                "universe_id": "0",
                "description": "Test array",
                "lights": {
                    "center": "rgb:1,rgb:4",
                    "frame": "s:7",
                    "spot": "$2,w:100",
                    "all": "@center,@spot,@frame"
                },
                "dimmer_level": 1000,
                "effects": {
                    "on": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(255); rgb(255,255,255); w(255)"
                    },
                    "off": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(0); rgb(0,0,0); w(0)"
                    }
                },
                "values": {
                },
                "presets": [
                ]
            }"#;

        let array = serde_json::from_str::<DmxArray>(array_json).unwrap();
        array_manager.add_array("test".to_string(), array).unwrap();
        let scope = Scope::new(&array_manager, "test", None, None).unwrap();

        let result = scope.get_light_channels("@all").unwrap();
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

    }

    #[test]
    fn test_expand_values() {
        let mut array_manager = ArrayManager::new();
        let array_json = r#"
            {
                "universe_id": "0",
                "description": "Test array",
                "lights": {
                    "center": "rgb:1,rgb:4",
                    "frame": "s:7",
                    "spot": "$2,w:100",
                    "all": "@center,@spot,@frame"
                },
                "dimmer_level": 1000,
                "effects": {
                    "on": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(255); rgb(255,255,255); w(255)"
                    },
                    "off": {
                        "type": "fade",
                        "lights": "@all",
                        "ticks": 10,
                        "target": "s(0); rgb(0,0,0); w(0)"
                    }
                },
                "values": {
                    "test": "test-array-value",
                    "test2": "test2-array-value"
                },
                "presets": [
                    {
                        "description": "Test preset",
                        "values": {
                            "preset1-value": "preset1-value-value",
                            "test2": "test2-preset-value"
                        }
                    }
                ]
            }"#;

        let array = serde_json::from_str::<DmxArray>(array_json).unwrap();
        array_manager.add_array("test".to_string(), array).unwrap();

        let scope = Scope::new(&array_manager, "test", None, None).unwrap();
        let result = scope.expand_values("hello `test` world").unwrap();
        assert_eq!(result, "hello test-array-value world");

        let result = array_manager.expand_values(&scope, "hello `void=default` world").unwrap();
        assert_eq!(result, "hello default world");

        let scope = Scope::new(&&array_manager, "test", Some(0), None).unwrap();
        let result = scope.expand_values( "hello `test2` world").unwrap();
        assert_eq!(result, "hello test2-preset-value world");

        let scope = Scope::new(&array_manager, "test", Some(0), Some(HashMap::from([
            ("test".to_string(), "test-local-value".to_string())
        ]))).unwrap();

        let result = scope.expand_values("hello `test` world").unwrap();
        assert_eq!(result, "hello test-local-value world");

        let result = scope.expand_values("hello `NONE` world");
        assert!(result.is_err());

        if let Err(e) = result {
            let t = e.to_string();
            assert_eq!(t, "Array 'test' preset# 0 'hello `NONE` world' has no value for NONE");
        }

        let result = scope.expand_values("hello `NONE world");
        assert!(result.is_err());

        if let Err(e) = result {
            let t = e.to_string();
            assert_eq!(t, "Array 'test' 'hello `NONE world' has unterminated `value` expression");
        }

    }
}