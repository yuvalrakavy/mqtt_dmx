
use crate::defs;
use crate::array_manager::scope::Scope;
use crate::defs::TargetValue;
use crate::effects_manager::runtime_nodes;
use crate::effects_manager::EffectNodeRuntime;

use serde::Deserialize;

use super::DmxArrayError;


impl defs::EffectNodeDefinition {
    pub fn get_runtime_node(&self, scope: &Scope) -> Result<Box<dyn EffectNodeRuntime>, DmxArrayError> {
        match self {
            defs::EffectNodeDefinition::Sequence(node) => node.get_runtime_node(scope),
            defs::EffectNodeDefinition::Parallel(node) => node.get_runtime_node(scope),
            &defs::EffectNodeDefinition::Fade(ref node) => node.get_runtime_node(scope),
            &defs::EffectNodeDefinition::Delay(ref node) => node.get_runtime_node(scope),
        }
    }
}

impl defs::SequenceEffectNodeDefinition {
    fn get_runtime_node(&self, scope: &Scope) -> Result<Box<dyn EffectNodeRuntime>, DmxArrayError> {
        let nodes = self.nodes.iter().map(|node| node.get_runtime_node(scope)).collect::<Result<Vec<Box<dyn EffectNodeRuntime>>, DmxArrayError>>()?;
        Ok(Box::new(runtime_nodes::SequenceEffectNode {
            nodes,
            current_node: 0,
        }))
    }
}



impl defs::ParallelEffectNodeDefinition {
    fn get_runtime_node(&self, scope: &Scope) -> Result<Box<dyn EffectNodeRuntime>, DmxArrayError> {
        let nodes = self.nodes.iter().map(|node| node.get_runtime_node(scope)).collect::<Result<Vec<Box<dyn EffectNodeRuntime>>, DmxArrayError>>()?;
        
        Ok(Box::new(runtime_nodes::ParallelEffectNode {
            nodes,
        }))
    }
}

impl defs::DelayEffectNodeDefinition {
    fn get_runtime_node(&self, scope: &Scope) -> Result<Box<dyn EffectNodeRuntime>, DmxArrayError> {

        Ok(Box::new(runtime_nodes::DelayEffectNode {
            ticks: self.ticks.get_value(scope, "delay ticks parameter")?,
            current_tick: 0,
        }))
    }
}

impl defs::FadeEffectNodeDefinition {
    fn get_runtime_node(&self, scope: &Scope) -> Result<Box<dyn EffectNodeRuntime>, DmxArrayError> {
        let lights_list = scope.expand_values(&self.lights)?;
        let lights = scope.get_light_channels(&lights_list)?;
        let ticks = self.ticks.get_value(scope, "fade ticks parameter")?;
        let target  = scope.expand_values(&self.target)?.parse::<TargetValue>().
            map_err(|e| DmxArrayError::ValueError(scope.to_string(), "fade target parameter", e.to_string()))?;

        Ok(Box::new(runtime_nodes::FadeEffectNode {
            lights,
            ticks,
            target,
        }))
    }
}
