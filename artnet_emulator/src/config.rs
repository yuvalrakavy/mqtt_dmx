use serde::Deserialize;
use std::collections::HashSet;

use crate::state::{ChannelType, EmulatorState, LightDefinition, UniverseKey, UniverseState};

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_web_port")]
    pub web_port: u16,
    #[serde(default)]
    pub universes: Vec<UniverseConfig>,
}

fn default_port() -> u16 { 6454 }
fn default_web_port() -> u16 { 8080 }

#[derive(Debug, Deserialize)]
pub struct UniverseConfig {
    #[serde(default)]
    pub net: u8,
    #[serde(default)]
    pub subnet: u8,
    #[serde(default)]
    pub universe: u8,
    #[serde(default = "default_channels")]
    pub channels: u16,
    pub description: Option<String>,
    #[serde(default)]
    pub lights: Vec<LightConfig>,
}

fn default_channels() -> u16 { 512 }

#[derive(Debug, Deserialize)]
pub struct LightConfig {
    pub name: String,
    pub channels: String, // e.g. "rgb:1", "s:4", "w:5"
}

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        let mut seen = HashSet::new();
        for u in &self.universes {
            let key = (u.net, u.subnet, u.universe);
            if !seen.insert(key) {
                return Err(format!("Duplicate universe: net={} subnet={} universe={}", u.net, u.subnet, u.universe));
            }
            if u.channels == 0 || u.channels > 512 {
                return Err(format!("Invalid channel count {} for universe {:?}", u.channels, key));
            }
        }
        Ok(())
    }

    pub fn to_emulator_state(&self) -> EmulatorState {
        let mut state = EmulatorState::empty();

        for u_cfg in &self.universes {
            let key = UniverseKey {
                net: u_cfg.net,
                subnet: u_cfg.subnet,
                universe: u_cfg.universe,
            };

            let lights = if u_cfg.lights.is_empty() {
                crate::state::generate_default_lights(u_cfg.channels)
            } else {
                u_cfg.lights.iter().map(parse_light_config).collect()
            };

            let universe = UniverseState::new(key, u_cfg.channels, u_cfg.description.clone(), lights);
            state.universes.insert(key, universe);
        }

        state
    }
}

fn parse_light_config(light: &LightConfig) -> LightDefinition {
    let channel_type = parse_channel_string(&light.channels);
    LightDefinition {
        name: light.name.clone(),
        channel_type,
    }
}

/// Parse channel definition string: "rgb:1", "s:4", "w:5", "rgb:1/3/5"
fn parse_channel_string(s: &str) -> ChannelType {
    let colon = s.find(':');
    let (type_str, channel_str) = match colon {
        Some(c) => (s[..c].trim(), s[c + 1..].trim()),
        None => ("s", s.trim()),
    };

    let channels: Vec<u16> = channel_str
        .split('/')
        .filter_map(|c| c.trim().parse::<u16>().ok())
        .collect();

    match type_str.to_lowercase().as_str() {
        "rgb" => match channels.len() {
            1 => ChannelType::Rgb(channels[0], channels[0] + 1, channels[0] + 2),
            3 => ChannelType::Rgb(channels[0], channels[1], channels[2]),
            _ => ChannelType::Single(channels.first().copied().unwrap_or(1)),
        },
        "w" => match channels.len() {
            1 => ChannelType::TriWhite(channels[0], channels[0] + 1, channels[0] + 2),
            3 => ChannelType::TriWhite(channels[0], channels[1], channels[2]),
            _ => ChannelType::Single(channels.first().copied().unwrap_or(1)),
        },
        _ => ChannelType::Single(channels.first().copied().unwrap_or(1)),
    }
}
