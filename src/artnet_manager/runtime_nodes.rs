use error_stack::Result;
use super::manager::{ArtnetManager, EffectNodeRuntime};
use super::ArtnetError;
use crate::array_manager::{error::DmxArrayError, Scope};
use crate::defs;
use crate::defs::TargetValue;
use crate::dmx::{ChannelDefinition, ChannelValue, DimmerValue, UniverseChannelDefinitions};

#[derive(Debug)]
pub struct SequenceEffectNode {
    pub nodes: Vec<Box<dyn EffectNodeRuntime>>,
    pub current_node: usize,
}

impl defs::SequenceEffectNodeDefinition {
    pub fn get_runtime_node(
        &self,
        scope: &Scope,
    ) -> Result<Box<dyn EffectNodeRuntime>, DmxArrayError> {
        let nodes = self
            .nodes
            .iter()
            .map(|node| node.get_runtime_node(scope))
            .collect::<Result<Vec<Box<dyn EffectNodeRuntime>>, DmxArrayError>>()?;
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
    pub fn get_runtime_node(
        &self,
        scope: &Scope,
    ) -> Result<Box<dyn EffectNodeRuntime>, DmxArrayError> {
        let nodes = self
            .nodes
            .iter()
            .map(|node| node.get_runtime_node(scope))
            .collect::<Result<Vec<Box<dyn EffectNodeRuntime>>, DmxArrayError>>()?;

        Ok(Box::new(ParallelEffectNode { nodes }))
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
    pub fn get_runtime_node(
        &self,
        scope: &Scope,
    ) -> Result<Box<dyn EffectNodeRuntime>, DmxArrayError> {
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
    pub fn get_runtime_node(
        &self,
        scope: &Scope,
    ) -> Result<Box<dyn EffectNodeRuntime>, DmxArrayError> {
        let lights_list = scope.expand_values(&self.lights)?;
        let lights = scope.get_light_channels(&lights_list)?;
        let ticks = self.ticks.get_value(scope, "fade ticks parameter")?;
        let target = scope
            .expand_values(&self.target)?
            .parse::<TargetValue>()
            .map_err(|e| {
                DmxArrayError::ValueError(scope.to_string(), "fade target parameter", e.to_string())
            })?
            .get_dimmed_value(if self.no_dimming { defs::DIMMING_AMOUNT_MAX } else { scope.dimming_amount });

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
            let state = self.initialize_state(artnet_manager)?;

            if !state.fade_needed() {
                self.current_tick = self.ticks;
            } else {
                self.state = Some(state);
            }
        }

        if self.current_tick < self.ticks {
            for universe_state in self.state.as_mut().unwrap().universe_states.iter_mut() {
                for channel_state in universe_state.channel_states.iter_mut() {
                    channel_state.value.tick();
                    artnet_manager.set_channel(
                        &universe_state.universe_id,
                        &channel_state.get_channel_value(),
                    )?;
                }
            }
            self.current_tick += 1;
        }

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

impl FadeEffectState {
    pub(self) fn fade_needed(&self) -> bool {
        self.universe_states
            .iter()
            .any(|universe_state| universe_state.fade_needed())
    }
}

#[derive(Debug)]
struct FadeEffectUniverseState {
    universe_id: String,
    channel_states: Vec<FadeEffectChannelState>,
}

impl FadeEffectUniverseState {
    pub(self) fn fade_needed(&self) -> bool {
        self.channel_states
            .iter()
            .any(|channel_state| channel_state.is_fade_needed())
    }
}

#[derive(Debug)]
struct FadeEffectChannelState {
    pub channel: ChannelDefinition,
    pub value: FadeEffectDimmerState,
}

impl FadeEffectChannelState {
    fn get_channel_value(&self) -> ChannelValue {
        match &self.value {
            FadeEffectDimmerState::Single(v) => ChannelValue {
                channel: self.channel.clone(),
                value: DimmerValue::Single(v.value),
            },
            FadeEffectDimmerState::Rgb(r, g, b) => ChannelValue {
                channel: self.channel.clone(),
                value: DimmerValue::Rgb(r.value, g.value, b.value),
            },
            FadeEffectDimmerState::TriWhite(w1, w2, w3) => ChannelValue {
                channel: self.channel.clone(),
                value: DimmerValue::TriWhite(w1.value, w2.value, w3.value),
            },
        }
    }

    pub(self) fn is_fade_needed(&self) -> bool {
        self.value.is_fade_needed()
    }
}

#[derive(Debug)]
enum FadeEffectDimmerState {
    Single(DmxChannelDelta),
    Rgb(DmxChannelDelta, DmxChannelDelta, DmxChannelDelta),
    TriWhite(DmxChannelDelta, DmxChannelDelta, DmxChannelDelta),
}

impl FadeEffectDimmerState {
    pub(self) fn tick(&mut self) {
        match self {
            FadeEffectDimmerState::Single(channel) => {
                channel.tick();
            }
            FadeEffectDimmerState::Rgb(r, g, b) => {
                r.tick();
                g.tick();
                b.tick();
            }
            FadeEffectDimmerState::TriWhite(w1, w2, w3) => {
                w1.tick();
                w2.tick();
                w3.tick();
            }
        }
    }

    pub(self) fn is_fade_needed(&self) -> bool {
        match self {
            FadeEffectDimmerState::Single(channel) => channel.is_fade_needed(),
            FadeEffectDimmerState::Rgb(r, g, b) => {
                r.is_fade_needed() || g.is_fade_needed() || b.is_fade_needed()
            }
            FadeEffectDimmerState::TriWhite(w1, w2, w3) => {
                w1.is_fade_needed() || w2.is_fade_needed() || w3.is_fade_needed()
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
            fraction,
        }
    }

    pub fn tick(&mut self) {
        if self.is_increment {
            self.value += self.delta;
            if self.fraction >= 0 {
                self.value += 1;
                self.fraction -= self.dx as i32;
            }
        } else {
            self.value -= self.delta;
            if self.fraction >= 0 {
                self.value -= 1;
                self.fraction -= self.dx as i32;
            }
        }

        self.fraction += self.dy as i32;
    }

    pub(self) fn is_fade_needed(&self) -> bool {
        self.delta > 0 || self.dy > 0
    }
}

impl FadeEffectNode {
    fn initialize_state(
        &self,
        artnet_manager: &mut ArtnetManager,
    ) -> Result<FadeEffectState, ArtnetError> {
        let mut universe_states = Vec::<FadeEffectUniverseState>::new();

        for universe in self.lights.iter() {
            universe_states.push(FadeEffectUniverseState {
                universe_id: universe.universe_id.clone(),
                channel_states: self.initialize_universe_state(artnet_manager, universe)?,
            });
        }

        Ok(FadeEffectState { universe_states })
    }

    fn initialize_universe_state(
        &self,
        artnet_manager: &mut ArtnetManager,
        universe: &UniverseChannelDefinitions,
    ) -> Result<Vec<FadeEffectChannelState>, ArtnetError> {
        let mut channel_states = Vec::<FadeEffectChannelState>::new();

        for channel in universe.channels.iter() {
            if let Some(channel_state) =
                self.initialize_channel_state(artnet_manager, &universe.universe_id, channel)?
            {
                channel_states.push(channel_state);
            }
        }

        Ok(channel_states)
    }

    fn initialize_channel_state(
        &self,
        artnet_manager: &mut ArtnetManager,
        universe_id: &str,
        channel_definition: &ChannelDefinition,
    ) -> Result<Option<FadeEffectChannelState>, ArtnetError> {
        Ok(
            match artnet_manager
                .get_channel(universe_id, channel_definition)?
                .value
            {
                DimmerValue::Rgb(current_r, current_g, current_b) => {
                    self.target.rgb.map(|target| FadeEffectChannelState {
                        channel: channel_definition.clone(),
                        value: FadeEffectDimmerState::Rgb(
                            DmxChannelDelta::new(current_r, target.0, self.ticks),
                            DmxChannelDelta::new(current_g, target.1, self.ticks),
                            DmxChannelDelta::new(current_b, target.2, self.ticks),
                        ),
                    })
                }
                DimmerValue::TriWhite(current_w1, current_w2, current_w3) => {
                    self.target.tri_white.map(|target| FadeEffectChannelState {
                        channel: channel_definition.clone(),
                        value: FadeEffectDimmerState::TriWhite(
                            DmxChannelDelta::new(current_w1, target.0, self.ticks),
                            DmxChannelDelta::new(current_w2, target.1, self.ticks),
                            DmxChannelDelta::new(current_w3, target.2, self.ticks),
                        ),
                    })
                }
                DimmerValue::Single(current) => {
                    self.target.single.map(|target| FadeEffectChannelState {
                        channel: channel_definition.clone(),
                        value: FadeEffectDimmerState::Single(DmxChannelDelta::new(
                            current, target, self.ticks,
                        )),
                    })
                }
            },
        )
    }
}
