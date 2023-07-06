
use std::collections::HashMap;
use std::fmt::Display;

use super::manager::ArrayManager;
use super::error::DmxArrayError;
use crate::dmx::{UniverseChannelDefinitions, ChannelDefinition};
use crate::defs::DmxArray;

impl UniverseChannelDefinitions {
    pub (super) fn new(universe_id: String) -> Self {
        Self {
            universe_id,
            channels: Vec::new(),
        }
    }

    pub (super) fn add(&mut self, channel: ChannelDefinition) {
        self.channels.push(channel);
    }
}

pub (super) struct ExpansionStack {
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
    pub (super) fn static_do_get_array_light_channels(array_id: &str, array: &DmxArray, lights_list: &str, result: &mut HashMap<String, UniverseChannelDefinitions>, stack: &mut ExpansionStack) -> Result<(), DmxArrayError> {
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

    pub (super) fn static_get_array_light_channels(array_id: &str, array: &DmxArray, lights_list: &str) -> Result<Vec<UniverseChannelDefinitions>, DmxArrayError> {
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
}
