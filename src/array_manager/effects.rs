use std::sync::Arc;
use error_stack::Result;

use crate::defs::{self, DimmingAmount};
use crate::defs::{EffectNodeDefinition, EffectUsage};

use super::error::DmxArrayError;
use super::{ArrayManager, Scope};
use crate::artnet_manager::EffectNodeRuntime;

impl defs::EffectNodeDefinition {
    pub fn get_runtime_node(
        &self,
        scope: &Scope,
    ) -> Result<Box<dyn EffectNodeRuntime>, DmxArrayError> {
        match self {
            defs::EffectNodeDefinition::Sequence(node) => node.get_runtime_node(scope),
            defs::EffectNodeDefinition::Parallel(node) => node.get_runtime_node(scope),
            defs::EffectNodeDefinition::Fade(ref node) => node.get_runtime_node(scope),
            defs::EffectNodeDefinition::Delay(ref node) => node.get_runtime_node(scope),
        }
    }
}

impl ArrayManager {
    pub(super) fn add_effect(&mut self, effect_id: Arc<str>, effect: EffectNodeDefinition) -> Result<(), DmxArrayError> {
        self.effects.insert(effect_id, effect);
        Ok(())
    }

    pub(super) fn remove_effect(&mut self, effect_id: &str) -> Result<(), DmxArrayError> {
        self.effects.remove(effect_id);
        Ok(())
    }

    //
    // Get effect definition by looking for the effect_id in the array effects list, then the global effects list.
    // If the effect_id is not found, return None.
    //
    fn get_effect_definition(
        &self,
        array_id: &str,
        effect_id: &str,
    ) -> Result<Option<&EffectNodeDefinition>, DmxArrayError> {
        let array = self.get_array(array_id)?;
        Ok(array
            .effects
            .get(effect_id)
            .or_else(|| self.effects.get(effect_id)))
    }

    fn get_usage_effect_id(
        &self,
        usage: &EffectUsage,
        array_id: &str,
        effect_id: Option<&Arc<str>>
    ) -> Result<Arc<str>, DmxArrayError> {
        if let Some(effect_id) = effect_id {
            Ok(effect_id.clone())
        } else {
            let array = self.get_array(array_id)?;

            Ok(match usage {
                EffectUsage::On => array.on.clone(),
                EffectUsage::Off => array.off.clone(),
                &EffectUsage::Dim => array.dim.clone(),
            })
        }
    }

    pub(super) fn get_usage_effect_definition(
        &self,
        usage: &EffectUsage,
        array_id: &str,
        effect_id: Option<&Arc<str>>,
    ) -> Result<&EffectNodeDefinition, DmxArrayError> {
        let effect_id = self.get_usage_effect_id(usage, array_id, effect_id)?;
        let array = self.get_array(array_id)?;

        let effect_definition = self
            .get_effect_definition(array_id, &effect_id)?
            .or_else(|| match usage {
                EffectUsage::On if effect_id == array.on => Some(&self.default_on_effect),
                EffectUsage::Off if effect_id == array.off => Some(&self.default_off_effect),
                &EffectUsage::Dim if effect_id == array.dim => Some(&self.default_dim_effect),
                _ => None,
            });

        effect_definition.ok_or_else(|| {
            DmxArrayError::EffectNotFound(
                Arc::from(format!("{} ({})", array_id, array.description)),
                effect_id.clone(),
            ).into()
        })
    }

    pub fn get_usage_effect_runtime(
        &self,
        usage: &EffectUsage,
        array_id: &str,
        effect_id: Option<&Arc<str>>,
        dimming_amount: DimmingAmount,
    ) -> Result<Box<dyn EffectNodeRuntime>, DmxArrayError> {
        let effect_definition = self.get_usage_effect_definition(usage, array_id, effect_id)?;
        let scope = super::Scope::new(self, Arc::from(array_id), effect_id, dimming_amount)?;

        effect_definition.get_runtime_node(&scope)
    }
}
