use serde::Deserialize;
use std::net::IpAddr;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
pub struct UniverseDefinition {
    pub description: String,

    pub controller: IpAddr,
    pub net: u8,
    pub subnet: u8,
    pub universe: u8,
    pub channels: u16,
}

#[derive(Debug, Deserialize)]
enum EffectReference {
    EffectName(String),
    Effect(EffectNodeDefinition),
}

#[derive(Debug, Deserialize)]
pub struct DmxArray {
    pub description: String,
    
    lights: HashMap<String, String>,
    effects: HashMap<String, EffectNodeDefinition>,
    values: HashMap<String, String>,
    presets: Vec<DmxArrayPreset>,
}

/// Dmx Array Preset
#[derive(Debug, Deserialize)]
pub struct DmxArrayPreset {
    description: String,

    fade_in: EffectReference,
    fade_out: EffectReference,
}

/// Effect
#[derive(Debug, Deserialize)]
struct Effect {
    values: Option<HashMap<String, String>>,
    fade_in: Option<EffectReference>,
    fade_out: Option<EffectReference>,
}

/// Effect modes
#[derive(Deserialize, Debug)]
pub enum EffectNodeDefinition {
    Sequence(SequenceEffectNodeDefinition),
    Parallel(ParallelEffectNodeDefinition),
}

#[derive(Deserialize, Debug)]
pub struct SequenceEffectNodeDefinition {
    pub nodes: Vec<EffectNodeDefinition>,
}

#[derive(Deserialize, Debug)]
pub struct ParallelEffectNodeDefinition {
    pub nodes: Vec<EffectNodeDefinition>,
}
