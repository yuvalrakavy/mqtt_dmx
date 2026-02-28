use serde::{Deserialize, Serialize};

use crate::state::ChannelType;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EmulatorSnapshot {
    pub config_path: Option<String>,
    pub universes: Vec<UniverseSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UniverseSnapshot {
    pub net: u8,
    pub subnet: u8,
    pub universe: u8,
    pub label: String,
    pub description: Option<String>,
    pub channel_count: u16,
    pub channels: Vec<u8>,
    pub lights: Vec<LightSnapshot>,
    pub packet_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LightSnapshot {
    pub name: String,
    pub light_type: LightTypeSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LightTypeSnapshot {
    Single { channel: u16, value: u8 },
    Rgb { channels: (u16, u16, u16), r: u8, g: u8, b: u8 },
    TriWhite { channels: (u16, u16, u16), w1: u8, w2: u8, w3: u8 },
}

#[cfg(feature = "server")]
pub fn build_snapshot(state: &crate::state::EmulatorState) -> EmulatorSnapshot {
    let mut universes: Vec<UniverseSnapshot> = state
        .universes
        .values()
        .map(|u| {
            let lights: Vec<LightSnapshot> = u
                .lights
                .iter()
                .map(|l| {
                    let light_type = match &l.channel_type {
                        ChannelType::Single(ch) => {
                            let value = get_channel(&u.channels, *ch);
                            LightTypeSnapshot::Single { channel: *ch, value }
                        }
                        ChannelType::Rgb(r, g, b) => LightTypeSnapshot::Rgb {
                            channels: (*r, *g, *b),
                            r: get_channel(&u.channels, *r),
                            g: get_channel(&u.channels, *g),
                            b: get_channel(&u.channels, *b),
                        },
                        ChannelType::TriWhite(w1, w2, w3) => LightTypeSnapshot::TriWhite {
                            channels: (*w1, *w2, *w3),
                            w1: get_channel(&u.channels, *w1),
                            w2: get_channel(&u.channels, *w2),
                            w3: get_channel(&u.channels, *w3),
                        },
                    };
                    LightSnapshot {
                        name: l.name.clone(),
                        light_type,
                    }
                })
                .collect();

            UniverseSnapshot {
                net: u.key.net,
                subnet: u.key.subnet,
                universe: u.key.universe,
                label: u.key.to_string(),
                description: u.description.clone(),
                channel_count: u.channel_count,
                channels: u.channels[..u.channel_count as usize].to_vec(),
                lights,
                packet_count: u.packet_count,
            }
        })
        .collect();

    universes.sort_by_key(|u| (u.net, u.subnet, u.universe));

    EmulatorSnapshot {
        config_path: state.config_path.clone(),
        universes,
    }
}

#[cfg(feature = "server")]
fn get_channel(channels: &[u8; 512], ch: u16) -> u8 {
    if ch == 0 || ch > 512 {
        0
    } else {
        channels[(ch - 1) as usize] // DMX channels are 1-indexed
    }
}
