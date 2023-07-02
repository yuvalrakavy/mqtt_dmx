
use std::collections::HashMap;
use std::fmt::Display;

use super::manager::ArrayManager;
use super::error::DmxArrayError;
use crate::defs::DmxArray;
use crate::dmx::{ChannelType, UniverseChannelDefinitions};

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
        self.verify_array_effects(array_id, array)?;
        Self::verify_array_lights(array_id, array)?;
        Ok(())
    }

    pub (super) fn verify_array_effects(&self, array_id: &str, array: &DmxArray) -> Result<(), DmxArrayError> {
        let has_effect = |effect_name: &str| {
            array.effects.get(effect_name).is_some() || self.effects.get(effect_name).is_some()
        };

        let has_on_effect = array
            .effects
            .get("on")
            .or_else(|| self.effects.get("on"))
            .is_some();
        let has_off_effect = array
            .effects
            .get("off")
            .or_else(|| self.effects.get("off"))
            .is_some();

        if array.presets.is_empty() && !has_on_effect && !has_off_effect {
            return Err(DmxArrayError::ArrayNoDefaultEffects(array_id.to_string()));
        }

        for (preset_number, preset) in array.presets.iter().enumerate() {
            if let Some(ref effect_name) = preset.on {
                if !has_effect(effect_name) {
                    return Err(DmxArrayError::ArrayPresetEffectNotFound(
                        array_id.to_string(),
                        preset_number,
                        "on",
                        effect_name.to_string(),
                    ));
                }
            } else if !has_on_effect {
                return Err(DmxArrayError::ArrayPresetDefaultEffectNotFound(
                    array_id.to_string(),
                    preset_number,
                    "on",
                ));
            }

            if let Some(ref effect_name) = preset.off {
                if !has_effect(effect_name) {
                    return Err(DmxArrayError::ArrayPresetEffectNotFound(
                        array_id.to_string(),
                        preset_number,
                        "off",
                        effect_name.to_string(),
                    ));
                }
            } else if !has_on_effect {
                return Err(DmxArrayError::ArrayPresetDefaultEffectNotFound(
                    array_id.to_string(),
                    preset_number,
                    "off",
                ));
            }
        }

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
                    match channel_definition.channel_type {
                        ChannelType::Single => {
                            add_channel_usage(channel_definition.channel, ChannelUsage::S)?
                        }
                        ChannelType::Rgb => {
                            add_channel_usage(channel_definition.channel, ChannelUsage::R)?;
                            add_channel_usage(channel_definition.channel + 1, ChannelUsage::G)?;
                            add_channel_usage(channel_definition.channel + 2, ChannelUsage::B)?;
                        }
                        ChannelType::TriWhite => {
                            add_channel_usage(channel_definition.channel, ChannelUsage::W1)?;
                            add_channel_usage(channel_definition.channel + 1, ChannelUsage::W2)?;
                            add_channel_usage(channel_definition.channel + 2, ChannelUsage::W3)?;
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
}
