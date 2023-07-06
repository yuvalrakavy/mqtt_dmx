
use crate::dmx::{UniverseChannelDefinitions, ChannelDefinition, ChannelValue, DimmerValue};
use crate::defs;
use crate::array_manager::{error::DmxArrayError, Scope};
use crate::defs::TargetValue;
use super::ArtnetError;
use super::manager::{EffectNodeRuntime, ArtnetManager};

#[derive(Debug)]
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
    fn tick(&mut self, artnet_manager: &mut ArtnetManager) -> Result<(), ArtnetError> {
        if self.current_node < self.nodes.len() {
            self.nodes[self.current_node].tick(artnet_manager)?;
            if self.nodes[self.current_node].is_done() {
                self.current_node += 1;
            }
        }

        Ok(())
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

#[derive(Debug)]
pub struct ParallelEffectNode {
    pub nodes: Vec<Box<dyn EffectNodeRuntime>>,
}

impl EffectNodeRuntime for ParallelEffectNode {
    fn tick(&mut self, artnet_manager: &mut ArtnetManager) -> Result<(), ArtnetError> {
        for node in self.nodes.iter_mut() {
            node.tick(artnet_manager)?;
        }

        Ok(())
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

#[derive(Debug)]
pub struct DelayEffectNode {
    pub ticks: usize,
    pub current_tick: usize,
}

impl EffectNodeRuntime for DelayEffectNode {
    fn tick(&mut self, _: &mut ArtnetManager) -> Result<(), ArtnetError> {
        if self.current_tick < self.ticks {
            self.current_tick += 1;
        }

        Ok(())
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
            map_err(|e| DmxArrayError::ValueError(scope.to_string(), "fade target parameter", e.to_string()))?.get_dimmed_value(scope.dimming_amount);

        Ok(Box::new(FadeEffectNode {
            lights,
            ticks,
            current_tick: 0,
            target,
            state: None,
        }))
    }
}

#[derive(Debug)]
pub struct FadeEffectNode {
    pub lights: Vec<UniverseChannelDefinitions>,
    pub ticks: usize,
    pub current_tick: usize,
    pub target: TargetValue,
    state: Option<FadeEffectState>,
}

impl EffectNodeRuntime for FadeEffectNode {
    fn tick(&mut self, artnet_manager: &mut ArtnetManager) -> Result<(), ArtnetError> {
        if self.state.is_none() {
            self.state = Some(self.initialize_state(artnet_manager)?);
        }

        for universe_state in self.state.as_mut().unwrap().universe_states.iter_mut() {
            for channel_state in universe_state.channel_states.iter_mut() {
                channel_state.value.tick();
                artnet_manager.set_channel(&universe_state.universe_id, &channel_state.get_channel_value())?;
            }
        }

        self.current_tick += 1;
        Ok(())
    }

    fn is_done(&self) -> bool {
        self.current_tick >= self.ticks
    }
}

#[derive(Debug)]
struct FadeEffectState {
    universe_states: Vec<FadeEffectUniverseState>,
}

#[derive(Debug)]
struct FadeEffectUniverseState {
    universe_id: String,
    channel_states: Vec<FadeEffectChannelState>,
}

#[derive(Debug)]
struct FadeEffectChannelState {
    pub channel: u16,
    pub value: FadeEffectDimmerState,
}

impl FadeEffectChannelState {
    fn get_channel_value(&self) -> ChannelValue {
        match &self.value {
            FadeEffectDimmerState::Single(v) => ChannelValue { channel: self.channel, value: DimmerValue::Single(v.value) },
            FadeEffectDimmerState::Rgb(r, g, b) => ChannelValue { channel: self.channel, value: DimmerValue::Rgb(r.value, g.value, b.value) },
            FadeEffectDimmerState::TriWhite(w1, w2, w3) => ChannelValue { channel: self.channel, value: DimmerValue::TriWhite(w1.value, w2.value, w3.value) },
        }
    }
} 


#[derive(Debug)]
enum FadeEffectDimmerState {
    Single(DmxChannelDelta),
    Rgb(DmxChannelDelta, DmxChannelDelta, DmxChannelDelta),
    TriWhite(DmxChannelDelta, DmxChannelDelta, DmxChannelDelta) 
}

impl FadeEffectDimmerState {
    pub fn tick(&mut self) {
        match self {
            FadeEffectDimmerState::Single(channel) => {
                channel.tick();
            },
            FadeEffectDimmerState::Rgb(r, g, b) => {
                r.tick();
                g.tick();
                b.tick();
            },
            FadeEffectDimmerState::TriWhite(w1, w2, w3) => {
                w1.tick();
                w2.tick();
                w3.tick();
            }
        }
    }
}

#[derive(Debug)]
struct DmxChannelDelta {
    pub value: u8,
    is_increment: bool,
    delta: u8,
    dx: u32,
    dy: u32,
    fraction: i32,
}

impl DmxChannelDelta {
    pub fn new(current_value: u8, target_value: u8, ticks: usize) -> DmxChannelDelta {
        let is_increment = target_value > current_value;
        let delta = if is_increment {
            (target_value - current_value) as usize / ticks
        } else {
            (current_value - target_value) as usize / ticks
        } as u8;
        let left_over = if is_increment {
            (target_value - current_value) as usize - delta as usize * ticks
        } else {
            (current_value - target_value) as usize - delta as usize * ticks
        } as u8;

        let dx = ticks as u32 * 2;
        let dy = left_over as u32 * 2;
        let fraction: i32 = dy as i32 - (dx as i32 >> 1);

        DmxChannelDelta { 
            value: current_value,
            is_increment,
            delta,
            dx,
            dy,
            fraction
        }
    }

    pub fn tick(&mut self)  {
        if self.is_increment {
            self.value += self.delta;
            if self.fraction >= 0 {
                self.value += 1;
                self.fraction -= self.dx as i32;
            }
        }
        else {
            self.value -= self.delta;
            if self.fraction >= 0 {
                self.value -= 1;
                self.fraction -= self.dx as i32;
            }
        }

        self.fraction += self.dy as i32;
    }
}

impl FadeEffectNode {
    fn initialize_state(&self, artnet_manager: &mut ArtnetManager) -> Result<FadeEffectState, ArtnetError> {
        let mut universe_states = Vec::<FadeEffectUniverseState>::new();

        for universe in self.lights.iter() {
                universe_states.push(FadeEffectUniverseState {
                    universe_id: universe.universe_id.clone(),
                    channel_states: self.initialize_universe_state(artnet_manager, universe)?
                });
        }

        Ok(FadeEffectState {
            universe_states,
        })
    }

    fn initialize_universe_state(&self, artnet_manager: &mut ArtnetManager, universe: &UniverseChannelDefinitions) -> Result<Vec<FadeEffectChannelState>, ArtnetError> {
        let mut channel_states = Vec::<FadeEffectChannelState>::new();

        for channel in universe.channels.iter() {
            if let Some(channel_state) = self.initialize_channel_state(artnet_manager, &universe.universe_id, channel)? {
                channel_states.push(channel_state);
            }
        }

        Ok(channel_states)
    }

    fn initialize_channel_state(&self, artnet_manager: &mut ArtnetManager, universe_id: &str, channel_definition: &ChannelDefinition) -> Result<Option<FadeEffectChannelState>, ArtnetError> {
        Ok(match artnet_manager.get_channel(universe_id, channel_definition)?.value {
            crate::dmx::DimmerValue::Rgb(current_r, current_g, current_b) => {
                if let Some((target_r, target_g, target_b)) = self.target.rgb {
                    Some(FadeEffectChannelState { channel: channel_definition.channel, value: FadeEffectDimmerState::Rgb(
                        DmxChannelDelta::new(current_r, target_r, self.ticks),
                        DmxChannelDelta::new(current_g, target_g, self.ticks),
                        DmxChannelDelta::new(current_b, target_b, self.ticks)
                    )})
                }
                else {
                    None
                }
            },
            crate::dmx::DimmerValue::TriWhite(current_w1, current_w2, current_w3) => {
                if let Some((target_w1, target_w2, target_w3)) = self.target.tri_white {
                    Some(FadeEffectChannelState { channel: channel_definition.channel, value: FadeEffectDimmerState::TriWhite(
                        DmxChannelDelta::new(current_w1, target_w1, self.ticks),
                        DmxChannelDelta::new(current_w2, target_w2, self.ticks),
                        DmxChannelDelta::new(current_w3, target_w3, self.ticks)
                    )})
                }
                else {
                    None
                }
            },
            crate::dmx::DimmerValue::Single(current) => {
                if let Some(target) = self.target.single {
                    Some(FadeEffectChannelState { channel: channel_definition.channel, value: FadeEffectDimmerState::Single(
                        DmxChannelDelta::new(current, target, self.ticks)
                    )})
                }
                else {
                    None
                }
            },
        })
    }
}