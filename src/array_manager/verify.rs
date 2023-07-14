
use std::collections::HashMap;
use std::fmt::Display;

use super::manager::ArrayManager;
use super::error::DmxArrayError;
use crate::defs::DmxArray;
use crate::dmx::{UniverseChannelDefinitions, ChannelDefinition};

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

impl ArrayManager {
    pub (super) fn verify_array(&self, array_id: &str, array: &DmxArray) -> Result<(), DmxArrayError> {
        Self::verify_array_lights(array_id, array)?;
        Ok(())
    }

    pub (super) fn verify_array_lights(array_id: &str, array: &DmxArray) -> Result<(), DmxArrayError> {
        let add_light_usage = |group_name: &str,
                               channel_usage: &mut HashMap<String, HashMap<u16, ChannelUsage>>,
                               must_exist: bool,
                               lights: Vec<UniverseChannelDefinitions>|
         -> Result<(), DmxArrayError> {
            for universe_channel_definition in lights.iter() {
                let universe_usage = channel_usage
                    .entry(universe_channel_definition.universe_id.clone())
                    .or_insert_with(HashMap::new);
                let mut add_channel_usage =
                    |channel: u16, usage: ChannelUsage| -> Result<(), DmxArrayError> {
                        if let Some(existing_usage) = universe_usage.get(&channel) {
                            if *existing_usage != usage {
                                return Err(DmxArrayError::ArrayLightChannelUsageMismatch(
                                    array_id.to_string(),
                                    universe_channel_definition.universe_id.clone(),
                                    channel,
                                    *existing_usage,
                                    usage,
                                    group_name.to_string(),
                                ));
                            }
                        } else if must_exist {
                            return Err(DmxArrayError::ArrayLightChannelNotInAllGroup(
                                array_id.to_string(),
                                universe_channel_definition.universe_id.clone(),
                                channel,
                                usage,
                                group_name.to_string(),
                            ));
                        } else {
                            universe_usage.insert(channel, usage);
                        }

                        Ok(())
                    };

                for channel_definition in universe_channel_definition.channels.iter() {
                    match channel_definition {
                        ChannelDefinition::Single(s) => {
                            add_channel_usage(*s, ChannelUsage::S)?
                        }
                        ChannelDefinition::Rgb(r, g, b) => {
                            add_channel_usage(*r, ChannelUsage::R)?;
                            add_channel_usage(*g, ChannelUsage::G)?;
                            add_channel_usage(*b, ChannelUsage::B)?;
                        }
                        ChannelDefinition::TriWhite(w1, w2, w3) => {
                            add_channel_usage(*w1, ChannelUsage::W1)?;
                            add_channel_usage(*w2, ChannelUsage::W2)?;
                            add_channel_usage(*w3, ChannelUsage::W3)?;
                        }
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

    // pub (super) fn verify_effects(&self, array_id: Option<&str>) -> Result<(), DmxArrayError> {
    //     let (effects, description) = if let Some(array_id) = array_id {
    //         if let Some(array) = self.arrays.get(array_id) {
    //             (&array.effects, format!("array {} ({})", array_id, array.description))
    //         } else {
    //             return Err(DmxArrayError::ArrayNotFound(array_id.to_string()));
    //         }
    //     } else {
    //         (&self.effects, "global".to_string())
    //     };

    //     let scope = Scope::new(&self, array_id, 
    //     for (effect_name, effect) in effects.iter() {
    //         _ =  effect.get_runtime_node()?;
    //     }

    //     Ok(())
    // }
}
