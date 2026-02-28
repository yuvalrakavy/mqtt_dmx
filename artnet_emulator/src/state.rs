use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[cfg(feature = "server")]
use std::time::Instant;

/// Identifies a DMX universe by ArtNet addressing
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UniverseKey {
    pub net: u8,
    pub subnet: u8,
    pub universe: u8,
}

impl std::fmt::Display for UniverseKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Net:{} Sub:{} Uni:{}", self.net, self.subnet, self.universe)
    }
}

/// Channel type for logical light grouping
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChannelType {
    Single(u16),
    Rgb(u16, u16, u16),
    TriWhite(u16, u16, u16),
}

/// A named logical light mapped to DMX channels
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LightDefinition {
    pub name: String,
    pub channel_type: ChannelType,
}

/// Runtime state of a single DMX universe
pub struct UniverseState {
    pub key: UniverseKey,
    pub description: Option<String>,
    pub channel_count: u16,
    pub channels: [u8; 512],
    pub lights: Vec<LightDefinition>,
    #[cfg(feature = "server")]
    pub last_update: Option<Instant>,
    pub packet_count: u64,
}

impl UniverseState {
    pub fn new(key: UniverseKey, channel_count: u16, description: Option<String>, lights: Vec<LightDefinition>) -> Self {
        Self {
            key,
            description,
            channel_count,
            channels: [0u8; 512],
            lights,
            #[cfg(feature = "server")]
            last_update: None,
            packet_count: 0,
        }
    }

    /// Update channels from incoming ArtNet packet data
    pub fn update_channels(&mut self, data: &[u8]) {
        let len = data.len().min(512);
        self.channels[..len].copy_from_slice(&data[..len]);
        self.channel_count = self.channel_count.max(len as u16);
        self.packet_count += 1;
        #[cfg(feature = "server")]
        {
            self.last_update = Some(Instant::now());
        }
    }
}

/// Top-level emulator state
pub struct EmulatorState {
    pub universes: HashMap<UniverseKey, UniverseState>,
    pub config_path: Option<String>,
    pub version: u64,
}

impl EmulatorState {
    pub fn empty() -> Self {
        Self {
            universes: HashMap::new(),
            config_path: None,
            version: 0,
        }
    }

    /// Update a universe's channels. If the universe doesn't exist, create it with auto-generated lights.
    pub fn update_universe(&mut self, key: UniverseKey, data: &[u8]) {
        let universe = self.universes.entry(key).or_insert_with(|| {
            let channel_count = data.len() as u16;
            let lights = generate_default_lights(channel_count);
            UniverseState::new(key, channel_count, None, lights)
        });
        universe.update_channels(data);
        self.version += 1;
    }
}

/// Generate default lights with a mix of RGB, tri-white, and single channels
pub fn generate_default_lights(channel_count: u16) -> Vec<LightDefinition> {
    let mut lights = Vec::new();
    let mut ch: u16 = 1; // DMX channels are 1-indexed
    let mut rgb_count = 0u16;
    let mut white_count = 0u16;
    let mut single_count = 0u16;

    while ch <= channel_count {
        let remaining = channel_count - ch + 1;

        if remaining >= 3 {
            // Cycle: RGB, tri-white, single
            let cycle = (rgb_count + white_count + single_count) % 3;
            match cycle {
                0 => {
                    rgb_count += 1;
                    lights.push(LightDefinition {
                        name: format!("RGB {rgb_count}"),
                        channel_type: ChannelType::Rgb(ch, ch + 1, ch + 2),
                    });
                    ch += 3;
                }
                1 => {
                    white_count += 1;
                    lights.push(LightDefinition {
                        name: format!("White {white_count}"),
                        channel_type: ChannelType::TriWhite(ch, ch + 1, ch + 2),
                    });
                    ch += 3;
                }
                _ => {
                    single_count += 1;
                    lights.push(LightDefinition {
                        name: format!("Single {single_count}"),
                        channel_type: ChannelType::Single(ch),
                    });
                    ch += 1;
                }
            }
        } else {
            // Not enough channels for a 3-channel type, use singles
            single_count += 1;
            lights.push(LightDefinition {
                name: format!("Single {single_count}"),
                channel_type: ChannelType::Single(ch),
            });
            ch += 1;
        }
    }

    lights
}

/// State version notifier for UI long-polling
#[cfg(feature = "server")]
#[derive(Clone)]
pub struct StateVersionNotifier {
    tx: tokio::sync::watch::Sender<u64>,
}

#[cfg(feature = "server")]
impl Default for StateVersionNotifier {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "server")]
impl StateVersionNotifier {
    pub fn new() -> Self {
        let (tx, _) = tokio::sync::watch::channel(0u64);
        Self { tx }
    }

    pub fn notify(&self, version: u64) {
        let _ = self.tx.send(version);
    }

    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.tx.subscribe()
    }
}
