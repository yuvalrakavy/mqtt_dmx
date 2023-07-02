
use std::collections::HashMap;

use super::manager::ArrayManager;
use super::DmxArrayError;
use crate::dmx::UniverseChannelDefinitions;

#[derive(Debug)]
pub struct Scope<'a> {
    array_manager: &'a ArrayManager,
    pub array_id: String,
    pub preset_number: Option<usize>,
    pub values: Option<HashMap<String, String>>,
}

impl std::fmt::Display for Scope<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let array_description = self.array_manager.arrays.get(&self.array_id).map(|a| a.description.clone()).unwrap_or_else(|| "--UNDEFINED--".to_string());

        write!(f, "Array '{}' ({})", self.array_id, array_description)?;

        if let Some(preset_number) = self.preset_number {
            write!(f, " preset# {}", preset_number)?;
        }

        Ok(())
    }
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
