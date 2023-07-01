
use crate::defs;
use serde::Deserialize;


struct Context {

}

trait EffectNodeRuntime {
    fn tick(&mut self);
    fn is_done(&self) -> bool;

}

trait GetRuntimeNode {
    fn get_runtime_node(&self, context: &Context) -> Box<dyn EffectNodeRuntime>;
}

impl defs::EffectNodeDefinition {
    fn get_runtime_node(&self, context: &Context) -> Box<dyn EffectNodeRuntime> {
        match self {
            defs::EffectNodeDefinition::Sequence(node) => node.get_runtime_node(context),
            defs::EffectNodeDefinition::Parallel(node) => node.get_runtime_node(context),
            &defs::EffectNodeDefinition::Fade(ref node) => node.get_runtime_node(context),
        }
    }
}

impl defs::SequenceEffectNodeDefinition {
    fn get_runtime_node(&self, context: &Context) -> Box<dyn EffectNodeRuntime> {
        let nodes = self.nodes.iter().map(|node| node.get_runtime_node(context)).collect();
        Box::new(SequenceEffectNode {
            nodes,
            current_node: 0,
        })
    }
}

// Runtime effect nodes

struct SequenceEffectNode {
    nodes: Vec<Box<dyn EffectNodeRuntime>>,
    current_node: usize,
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
    fn get_runtime_node(&self, context: &Context) -> Box<dyn EffectNodeRuntime> {
        let nodes = self.nodes.iter().map(|node| node.get_runtime_node(context)).collect();
        Box::new(ParallelEffectNode {
            nodes,
        })
    }
}

impl defs::FadeEffectNodeDefinition {
    fn get_runtime_node(&self, context: &Context) -> Box<dyn EffectNodeRuntime> {
        Box::new(FadeEffectNode {

        })
    }
}

struct ParallelEffectNode {
    nodes: Vec<Box<dyn EffectNodeRuntime>>,
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

struct FadeEffectNode {

}

impl EffectNodeRuntime for FadeEffectNode {
    fn tick(&mut self) {

    }

    fn is_done(&self) -> bool {
        false
    }
}