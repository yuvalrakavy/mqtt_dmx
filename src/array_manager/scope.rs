
use std::sync::Arc;

use super::manager::ArrayManager;
use super::DmxArrayError;
use crate::dmx::UniverseChannelDefinitions;
use crate::defs::DimmingAmount;

#[derive(Debug)]
pub struct Scope<'a> {
    array_manager: &'a ArrayManager,
    pub array_id: Arc<str>,
    pub effect_id: Option<Arc<str>>,
    pub dimming_amount: DimmingAmount,
}

impl std::fmt::Display for Scope<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let array_description = self.array_manager.arrays.get(&self.array_id).map(|a| a.description.clone()).unwrap_or_else(|| "--UNDEFINED--".to_string());

        write!(f, "Array '{}' ({})", self.array_id, array_description)?;

        if let Some(ref effect_id) = self.effect_id {
            write!(f, " effect {}", effect_id)?;
        }

        Ok(())
    }
}

impl Scope<'_> {
    pub fn new<'a>(array_manager: &'a ArrayManager, array_id: Arc<str>, effect_id: Option<&Arc<str>>, dimming_amount: DimmingAmount) -> Result<Scope<'a>, DmxArrayError> {
        let array = array_manager.arrays.get(&array_id);

        if array.is_none() {
            return Err(DmxArrayError::ArrayNotFound(array_id));
        }

        Ok(Scope {
            array_manager,
            array_id,
            effect_id: effect_id.cloned(),
            dimming_amount,
        })
    }

    pub fn get_light_channels(&self, lights_list: &str) -> Result<Vec<UniverseChannelDefinitions>, DmxArrayError> {
        self.array_manager.get_array_light_channels(&self.array_id, lights_list)
    }

    pub fn expand_values(&self, unexpanded_value: &str) -> Result<String, DmxArrayError> {
        self.array_manager.expand_values(self.array_id.clone(), unexpanded_value)
    }
}
