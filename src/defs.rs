use serde::Deserialize;
use std::net::IpAddr;
use std::collections::HashMap;
use std::fmt::Debug;
use std::str::FromStr;
use std::sync::Arc;

#[derive(Debug, Deserialize, Clone)]
pub struct UniverseDefinition {
    pub description: String,

    pub controller: IpAddr,
    pub net: u8,
    pub subnet: u8,
    pub universe: u8,
    pub channels: u16,

    #[serde(default)]
    pub log: bool,              // Log SetChannel calls for testing (applicable only if #cfg(test)

    #[serde(default)]
    pub disable_send: bool,     // Disable sending DMX packets for testing
}

#[derive(Debug, Deserialize, Clone)]
pub struct ValueDefinition {
    pub value: Arc<str>,
}

pub type DimmingAmount = usize;
pub const DIMMING_AMOUNT_MAX: DimmingAmount = 1000;

pub type SymbolTable = HashMap<Arc<str>, String>;

#[derive(Debug, Deserialize)]
pub struct DmxArray {
    pub description: String,
    
    pub universe_id: String,        // Default universe to use
    pub lights: HashMap<String, String>,
    #[serde(default="default_on_effect_id")]
    pub on: Arc<str>,
    #[serde(default="default_off_effect_id")]
    pub off: Arc<str>,
    #[serde(default="default_dim_effect_id")]
    pub dim: Arc<str>,
    #[serde(default)]
    pub effects: HashMap<String, EffectNodeDefinition>,
    #[serde(default)]
    pub default_values: SymbolTable,
}

fn default_on_effect_id() -> Arc<str> {
    Arc::from("on")
}

fn default_off_effect_id() -> Arc<str> {
    Arc::from("off")
}

fn default_dim_effect_id() -> Arc<str> {
    Arc::from("dim")
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
    Dim,
}

impl FromStr for EffectUsage {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "On" => Ok(EffectUsage::On),
            "Off" => Ok(EffectUsage::Off),
            "Dim" => Ok(EffectUsage::Dim),
            _ => panic!("Invalid effect usage: {}", s),
        }
    }
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
    #[serde(default)]
    pub no_dimming: bool,    
}

// Commands
//
// Sent to:  DMX/Command/On
// or to: DMX/Command/Off
#[derive(Deserialize, Debug)]
pub struct OnOffCommandParameters {
    pub array_id: Arc<str>,
    pub effect_id: Option<Arc<str>>,
    pub dimming_amount: Option<DimmingAmount>,
    pub values: Option<SymbolTable>,
}

#[derive(Deserialize, Debug)]
pub struct StopCommandParameters {
    pub array_id: Arc<str>,
}

#[derive(Deserialize, Debug)]
pub struct SetChannelsParameters {
    pub universe_id: String,
    pub channels: String,
    pub target: String,
    pub dimming_amount: Option<DimmingAmount>,
}
