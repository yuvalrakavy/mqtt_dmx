
use crate::dmx::UniverseChannelDefinitions;
use crate::defs;
use crate::array_manager::{error::DmxArrayError, Scope};
use crate::defs::TargetValue;
use super::EffectNodeRuntime;

pub struct SequenceEffectNode {
    pub nodes: Vec<Box<dyn EffectNodeRuntime>>,
    pub current_node: usize,
}

impl defs::SequenceEffectNodeDefinition {
    pub fn get_runtime_node(&self, scope: &Scope) -> Result<Box<dyn EffectNodeRuntime>, DmxArrayError> {
        let nodes = self.nodes.iter().map(|node| node.get_runtime_node(scope)).collect::<Result<Vec<Box<dyn EffectNodeRuntime>>, DmxArrayError>>()?;
        Ok(Box::new(SequenceEffectNode {
            nodes,
            current_node: 0,
        }))
    }
}

impl EffectNodeRuntime for SequenceEffectNode {
    fn tick(&mut self) {
        if self.current_node < self.nodes.len() {
            self.nodes[self.current_node].tick();
            if self.nodes[self.current_node].is_done() {
                self.current_node += 1;
            }
        }
    }

    fn is_done(&self) -> bool {
        self.current_node >= self.nodes.len()
    }
}

impl defs::ParallelEffectNodeDefinition {
    pub fn get_runtime_node(&self, scope: &Scope) -> Result<Box<dyn EffectNodeRuntime>, DmxArrayError> {
        let nodes = self.nodes.iter().map(|node| node.get_runtime_node(scope)).collect::<Result<Vec<Box<dyn EffectNodeRuntime>>, DmxArrayError>>()?;
        
        Ok(Box::new(ParallelEffectNode {
            nodes,
        }))
    }
}

pub struct ParallelEffectNode {
    pub nodes: Vec<Box<dyn EffectNodeRuntime>>,
}

impl EffectNodeRuntime for ParallelEffectNode {
    fn tick(&mut self) {
        for node in self.nodes.iter_mut() {
            node.tick();
        }
    }

    fn is_done(&self) -> bool {
        self.nodes.iter().all(|node| node.is_done())
    }
}

impl defs::DelayEffectNodeDefinition {
    pub fn get_runtime_node(&self, scope: &Scope) -> Result<Box<dyn EffectNodeRuntime>, DmxArrayError> {
        Ok(Box::new(DelayEffectNode {
            ticks: self.ticks.get_value(scope, "delay ticks parameter")?,
            current_tick: 0,
        }))
    }
}

pub struct DelayEffectNode {
    pub ticks: usize,
    pub current_tick: usize,
}

impl EffectNodeRuntime for DelayEffectNode {
    fn tick(&mut self) {
        if self.current_tick < self.ticks {
            self.current_tick += 1;
        }
    }

    fn is_done(&self) -> bool {
        self.current_tick >= self.ticks
    }
}

impl defs::FadeEffectNodeDefinition {
    pub fn get_runtime_node(&self, scope: &Scope) -> Result<Box<dyn EffectNodeRuntime>, DmxArrayError> {
        let lights_list = scope.expand_values(&self.lights)?;
        let lights = scope.get_light_channels(&lights_list)?;
        let ticks = self.ticks.get_value(scope, "fade ticks parameter")?;
        let target  = scope.expand_values(&self.target)?.parse::<TargetValue>().
            map_err(|e| DmxArrayError::ValueError(scope.to_string(), "fade target parameter", e.to_string()))?;

        Ok(Box::new(FadeEffectNode {
            lights,
            ticks,
            target,
        }))
    }
}

pub struct FadeEffectNode {
    pub lights: Vec<UniverseChannelDefinitions>,
    pub ticks: usize,
    pub target: TargetValue,
}

impl EffectNodeRuntime for FadeEffectNode {
    fn tick(&mut self) {

    }

    fn is_done(&self) -> bool {
        false
    }
}
