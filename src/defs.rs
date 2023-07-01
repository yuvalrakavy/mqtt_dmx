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
pub struct DmxArray {
    pub description: String,
    
    pub universe_id: String,        // Default universe to use
    pub lights: HashMap<String, String>,
    pub effects: HashMap<String, EffectNodeDefinition>,
    pub values: HashMap<String, String>,
    pub presets: Vec<DmxArrayPreset>,
    pub dimmer_level: u16,        // Dimming level (0-1000) maps to (0-100% brightness)
}
/// Dmx Array Preset
#[derive(Debug, Deserialize)]
pub struct DmxArrayPreset {
    pub description: String,
    pub values: HashMap<String, String>,
    pub on: Option<String>,
    pub off: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct TargetValue {
    pub single: Option<u8>,
    pub rgb: Option<(u8, u8, u8)>,
    pub tri_white: Option<(u8, u8, u8)>,
}

/// Effect modes
#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum EffectNodeDefinition {
    Sequence(SequenceEffectNodeDefinition),
    Parallel(ParallelEffectNodeDefinition),
    Fade(FadeEffectNodeDefinition),
}

#[derive(Deserialize, Debug)]
pub struct SequenceEffectNodeDefinition {
    pub nodes: Vec<EffectNodeDefinition>,
}

#[derive(Deserialize, Debug)]
pub struct ParallelEffectNodeDefinition {
    pub nodes: Vec<EffectNodeDefinition>,
}

#[derive(Deserialize, Debug)]
pub struct DelayEffectNodeDefinition {
    pub ticks: u32,
}

#[derive(Deserialize, Debug)]
pub struct FadeEffectNodeDefinition {
    pub lights: String,
    pub ticks: u32,
    pub target: String,
}
