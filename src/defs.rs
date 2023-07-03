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
    #[serde(default="default_on_effect_id")]
    pub on: String,
    #[serde(default="default_off_effect_id")]
    pub off: String,
    #[serde(default)]
    pub effects: HashMap<String, EffectNodeDefinition>,
    #[serde(default)]
    pub values: HashMap<String, String>,
    #[serde(default)]
    pub presets: Vec<DmxArrayPreset>,
}

fn default_on_effect_id() -> String {
    "on".to_string()
}

fn default_off_effect_id() -> String {
    "off".to_string()
}
/// Dmx Array Preset
#[derive(Debug, Deserialize)]
pub struct DmxArrayPreset {
    #[serde(default)]
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

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum NumberOrVariable {
    Number(usize),
    Variable(String),
}

#[derive(Deserialize, Debug, Clone, Copy)]
pub enum EffectUsage {
    On,
    Off,
}

/// Effect modes
#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum EffectNodeDefinition {
    Sequence(SequenceEffectNodeDefinition),
    Parallel(ParallelEffectNodeDefinition),
    Delay(DelayEffectNodeDefinition),
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
    pub ticks: NumberOrVariable,
}

#[derive(Deserialize, Debug)]
pub struct FadeEffectNodeDefinition {
    pub lights: String,
    pub ticks: NumberOrVariable,
    pub target: String,
}
