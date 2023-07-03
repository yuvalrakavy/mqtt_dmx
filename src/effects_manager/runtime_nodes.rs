
use crate::dmx::UniverseChannelDefinitions;
use crate::defs::TargetValue;
use super::EffectNodeRuntime;

pub struct SequenceEffectNode {
    pub nodes: Vec<Box<dyn EffectNodeRuntime>>,
    pub current_node: usize,
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
