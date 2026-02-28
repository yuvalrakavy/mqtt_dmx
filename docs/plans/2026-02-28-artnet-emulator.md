# ArtNet Emulator Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a standalone Dioxus web app that receives ArtNet UDP packets and displays DMX channel state in real-time, for testing the mqtt_dmx bridge without hardware.

**Architecture:** A Dioxus fullstack app with a tokio UDP listener. Incoming ArtNet packets update shared state (`Arc<RwLock<EmulatorState>>`), and the web UI long-polls for changes via a `StateVersionNotifier` watch channel. Auto-discovers universes from incoming packets; optionally pre-configured via JSON.

**Tech Stack:** Dioxus 0.7 (fullstack), tokio (UDP), serde/serde_json, rustop (CLI), chrono (timestamps)

**Reference projects:**
- HDL emulator: `/Users/yuval/Documents/Projects/Home/mqtt_hdl/hdl_emulator/`
- Lutron emulator: `/Users/yuval/Documents/Projects/Home/mqtt_lutron/hwi_emulator/`

---

### Task 1: Scaffold the crate

Create the `artnet_emulator/` crate with Cargo.toml, Dioxus.toml, empty module files, and a minimal "Hello World" Dioxus app that compiles and serves a web page.

**Files:**
- Create: `artnet_emulator/Cargo.toml`
- Create: `artnet_emulator/Dioxus.toml`
- Create: `artnet_emulator/assets/main.css`
- Create: `artnet_emulator/src/main.rs`
- Create: `artnet_emulator/src/lib.rs`

**Step 1: Create Cargo.toml**

```toml
[package]
name = "artnet_emulator"
version = "0.1.0"
edition = "2021"

[dependencies]
dioxus = { version = "0.7", features = ["fullstack", "router"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"], optional = true }
tracing = "0.1"
rustop = { version = "1", optional = true }
chrono = { version = "0.4", optional = true }

[features]
default = ["server", "web"]
server = ["dioxus/server", "dep:tokio", "dep:rustop", "dep:chrono"]
web = ["dioxus/web"]
```

**Step 2: Create Dioxus.toml**

```toml
[application]
name = "artnet_emulator"
default_platform = "fullstack"
out_dir = "dist"
asset_dir = "assets"

[web.app]
title = "ArtNet DMX Emulator"

[web.watcher]
reload_html = true
watch_path = ["src", "assets"]

[web.resource.dev]
style = ["/main.css"]
script = []

[web.resource.release]
style = ["/main.css"]
script = []
```

**Step 3: Create assets/main.css**

An empty CSS file (or copy from the HDL emulator's Tailwind-like utility classes if available). Start with:

```css
/* ArtNet Emulator styles */
```

**Step 4: Create lib.rs**

```rust
pub mod config;
pub mod state;
pub mod udp;
pub mod ui;
```

**Step 5: Create main.rs with minimal Dioxus app**

Follow the exact pattern from the HDL emulator. For now, just get a "Hello World" page serving:

```rust
mod config;
mod state;
mod udp;
mod ui;

use dioxus::prelude::*;

static CSS: Asset = asset!("/assets/main.css");

fn main() {
    #[cfg(feature = "server")]
    {
        use rustop::opts;

        let (args, _rest) = opts! {
            synopsis "ArtNet DMX Emulator";
            opt config_path: Option<String>, desc: "Path to config JSON file", long: "config", short: 'c';
            opt port: u16 = 6454, desc: "ArtNet UDP port", long: "port", short: 'p';
            opt web_port: u16 = 8080, desc: "Port for the web UI", long: "web-port", short: 'w';
        }
        .parse_or_exit();

        println!("Starting ArtNet Emulator — UDP port: {}, Web port: {}", args.port, args.web_port);

        if std::env::var("PORT").is_err() {
            unsafe { std::env::set_var("PORT", args.web_port.to_string()) };
        }
        if std::env::var("IP").is_err() {
            unsafe { std::env::set_var("IP", "0.0.0.0") };
        }

        dioxus::launch(App);
    }

    #[cfg(not(feature = "server"))]
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    rsx! {
        document::Stylesheet { href: CSS }
        div { class: "min-h-screen bg-gray-900 text-gray-100 p-4",
            h1 { class: "text-2xl font-bold mb-4", "ArtNet DMX Emulator" }
            p { "Emulator starting..." }
        }
    }
}
```

**Step 6: Create placeholder modules**

Create `artnet_emulator/src/config.rs`:
```rust
// Config module - will be populated in Task 3
```

Create `artnet_emulator/src/state.rs`:
```rust
// State module - will be populated in Task 2
```

Create `artnet_emulator/src/udp/mod.rs`:
```rust
// UDP module - will be populated in Task 4
```

Create `artnet_emulator/src/ui/mod.rs`:
```rust
// UI module - will be populated in Task 6
```

**Step 7: Verify it compiles**

Run: `cd artnet_emulator && dx build`

If `dx` is not installed, install it: `cargo install dioxus-cli`

Expected: Builds successfully, serves a web page at `http://localhost:8080`

**Step 8: Commit**

```
git add artnet_emulator/
git commit -m "Scaffold artnet_emulator crate with minimal Dioxus app"
```

---

### Task 2: State management and version notifier

Implement `EmulatorState`, `UniverseState`, `UniverseKey`, `LightDefinition`, and `StateVersionNotifier`. These are the core data structures. No network code yet — just the state model.

**Files:**
- Modify: `artnet_emulator/src/state.rs`

**Step 1: Implement the state model**

```rust
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[cfg(feature = "server")]
use std::sync::Arc;
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
```

**Step 2: Verify it compiles**

Run: `cargo check --features server`
Expected: No errors

**Step 3: Commit**

```
git add artnet_emulator/src/state.rs
git commit -m "Add state model, version notifier, and default light generation"
```

---

### Task 3: Configuration and activity log

Implement JSON config loading and the shared activity log for the UI.

**Files:**
- Modify: `artnet_emulator/src/config.rs`
- Create: `artnet_emulator/src/udp/log.rs`
- Modify: `artnet_emulator/src/udp/mod.rs`

**Step 1: Implement config.rs**

```rust
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
                u_cfg.lights.iter().map(|l| parse_light_config(l)).collect()
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
/// Follows the same syntax as mqtt_dmx's ChannelDefinition::from_str
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
```

**Step 2: Implement udp/log.rs**

Follow the exact SharedLog pattern from the HDL emulator:

```rust
use std::collections::VecDeque;

#[cfg(feature = "server")]
use std::sync::Arc;
#[cfg(feature = "server")]
use tokio::sync::{watch, RwLock};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub message: String,
}

#[cfg(feature = "server")]
pub struct ActivityLog {
    entries: VecDeque<LogEntry>,
    capacity: usize,
}

#[cfg(feature = "server")]
impl ActivityLog {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push(&mut self, message: String) {
        if self.entries.len() >= self.capacity {
            self.entries.pop_front();
        }
        let timestamp = chrono::Local::now().format("%H:%M:%S%.3f").to_string();
        self.entries.push_back(LogEntry { timestamp, message });
    }

    pub fn entries(&self) -> &VecDeque<LogEntry> {
        &self.entries
    }
}

#[cfg(feature = "server")]
#[derive(Clone)]
pub struct SharedLog {
    buffer: Arc<RwLock<ActivityLog>>,
    version_tx: Arc<watch::Sender<u64>>,
}

#[cfg(feature = "server")]
impl SharedLog {
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = watch::channel(0u64);
        SharedLog {
            buffer: Arc::new(RwLock::new(ActivityLog::new(capacity))),
            version_tx: Arc::new(tx),
        }
    }

    pub async fn push(&self, message: String) {
        let mut buf = self.buffer.write().await;
        buf.push(message);
        self.version_tx.send_modify(|v| *v += 1);
    }

    pub async fn wait_and_get(&self, last_version: u64) -> (u64, VecDeque<LogEntry>) {
        let mut rx = self.version_tx.subscribe();
        loop {
            if *rx.borrow() != last_version {
                break;
            }
            if rx.changed().await.is_err() {
                break;
            }
        }
        let version = *rx.borrow();
        let buf = self.buffer.read().await;
        (version, buf.entries().clone())
    }
}
```

**Step 3: Update udp/mod.rs**

```rust
pub mod log;
pub mod server;
```

Create placeholder `artnet_emulator/src/udp/server.rs`:
```rust
// ArtNet UDP server - will be populated in Task 4
```

**Step 4: Verify it compiles**

Run: `cargo check --features server`
Expected: No errors

**Step 5: Commit**

```
git add artnet_emulator/src/config.rs artnet_emulator/src/udp/
git commit -m "Add config parsing and activity log"
```

---

### Task 4: ArtNet UDP server

Implement the UDP listener that receives ArtNet packets, parses them, updates state, and logs activity.

**Files:**
- Modify: `artnet_emulator/src/udp/server.rs`

**Step 1: Implement the ArtNet UDP server**

```rust
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::RwLock;

use crate::state::{EmulatorState, StateVersionNotifier, UniverseKey};
use crate::udp::log::SharedLog;

pub type SharedState = Arc<RwLock<EmulatorState>>;

const ARTNET_HEADER: &[u8; 8] = b"Art-Net\0";
const ARTNET_OPCODE_DMX: u16 = 0x5000;

/// Start the UDP server, returns the local address it bound to
pub async fn start_server(
    bind_addr: &str,
    state: SharedState,
    log: SharedLog,
    version_notifier: StateVersionNotifier,
) -> std::net::SocketAddr {
    let socket = UdpSocket::bind(bind_addr)
        .await
        .unwrap_or_else(|e| panic!("Failed to bind UDP socket to {bind_addr}: {e}"));
    let addr = socket.local_addr().unwrap();

    tracing::info!("ArtNet UDP server listening on {addr}");

    tokio::spawn(udp_server_loop(socket, state, log, version_notifier));

    addr
}

async fn udp_server_loop(
    socket: UdpSocket,
    state: SharedState,
    log: SharedLog,
    version_notifier: StateVersionNotifier,
) {
    let mut buf = [0u8; 1024];

    loop {
        let (len, src) = match socket.recv_from(&mut buf).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("UDP receive error: {e}");
                continue;
            }
        };

        let packet = &buf[..len];

        match parse_artnet_packet(packet) {
            Ok((key, dmx_data)) => {
                let channel_count = dmx_data.len();
                log.push(format!(
                    "From {src}: {} — {channel_count} channels",
                    key,
                )).await;

                let mut s = state.write().await;
                s.update_universe(key, dmx_data);
                version_notifier.notify(s.version);
            }
            Err(e) => {
                tracing::warn!("Invalid ArtNet packet from {src}: {e}");
            }
        }
    }
}

/// Parse an ArtNet DMX Output packet
/// Returns (UniverseKey, &[u8] DMX channel data)
fn parse_artnet_packet(packet: &[u8]) -> Result<(UniverseKey, &[u8]), &'static str> {
    // Minimum packet size: 8 (header) + 2 (opcode) + 2 (version) + 1 (seq) + 1 (phys) + 1 (sub_uni) + 1 (net) + 2 (length) = 18
    if packet.len() < 18 {
        return Err("Packet too short");
    }

    // Verify Art-Net header
    if &packet[0..8] != ARTNET_HEADER {
        return Err("Invalid Art-Net header");
    }

    // OpCode (little-endian at offset 8)
    let opcode = u16::from_le_bytes([packet[8], packet[9]]);
    if opcode != ARTNET_OPCODE_DMX {
        return Err("Not a DMX Output packet");
    }

    // SubUniverse: high nibble = subnet, low nibble = universe
    let sub_uni = packet[14];
    let subnet = (sub_uni >> 4) & 0x0F;
    let universe = sub_uni & 0x0F;
    let net = packet[15];

    // Data length (big-endian at offset 16)
    let data_len = u16::from_be_bytes([packet[16], packet[17]]) as usize;
    let data_end = 18 + data_len.min(packet.len() - 18);
    let dmx_data = &packet[18..data_end];

    let key = UniverseKey { net, subnet, universe };
    Ok((key, dmx_data))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_artnet_packet(net: u8, subnet: u8, universe: u8, data: &[u8]) -> Vec<u8> {
        let mut packet = Vec::new();
        packet.extend_from_slice(b"Art-Net\0"); // header
        packet.extend_from_slice(&0x5000u16.to_le_bytes()); // opcode
        packet.extend_from_slice(&[0x00, 0x0e]); // protocol version
        packet.push(0); // sequence
        packet.push(0); // physical
        packet.push((subnet << 4) | (universe & 0x0F)); // sub_uni
        packet.push(net); // net
        packet.extend_from_slice(&(data.len() as u16).to_be_bytes()); // length
        packet.extend_from_slice(data); // DMX data
        packet
    }

    #[test]
    fn test_parse_valid_packet() {
        let data = vec![255, 128, 0, 64];
        let packet = build_artnet_packet(0, 1, 2, &data);
        let (key, dmx_data) = parse_artnet_packet(&packet).unwrap();
        assert_eq!(key.net, 0);
        assert_eq!(key.subnet, 1);
        assert_eq!(key.universe, 2);
        assert_eq!(dmx_data, &[255, 128, 0, 64]);
    }

    #[test]
    fn test_parse_invalid_header() {
        let packet = b"Not-Art\0\x00\x50";
        assert!(parse_artnet_packet(packet).is_err());
    }

    #[test]
    fn test_parse_wrong_opcode() {
        let mut packet = build_artnet_packet(0, 0, 0, &[0]);
        packet[8] = 0x00; // change opcode to non-DMX
        packet[9] = 0x00;
        assert!(parse_artnet_packet(&packet).is_err());
    }

    #[test]
    fn test_parse_too_short() {
        let packet = b"Art-Net\0\x00\x50";
        assert!(parse_artnet_packet(packet).is_err());
    }
}
```

**Step 2: Run tests**

Run: `cargo test`
Expected: All 4 tests pass

**Step 3: Commit**

```
git add artnet_emulator/src/udp/server.rs
git commit -m "Add ArtNet UDP server with packet parsing and tests"
```

---

### Task 5: Wire up main.rs with shared state and UDP server

Connect the state, config, log, and UDP server in main.rs using the Dioxus `serve()` pattern with Axum extensions.

**Files:**
- Modify: `artnet_emulator/src/main.rs`

**Step 1: Full main.rs implementation**

Replace the placeholder main.rs with the full wiring:

```rust
mod config;
mod state;
mod udp;
mod ui;

use dioxus::prelude::*;

static CSS: Asset = asset!("/assets/main.css");

fn main() {
    #[cfg(feature = "server")]
    {
        use std::sync::Arc;
        use tokio::sync::RwLock;

        use dioxus::server::{DioxusRouterExt, ServeConfig};
        use rustop::opts;

        use crate::state::StateVersionNotifier;
        use crate::udp::log::SharedLog;

        let (args, _rest) = opts! {
            synopsis "ArtNet DMX Emulator";
            opt config_path: Option<String>, desc: "Path to config JSON file", long: "config", short: 'c';
            opt port: u16 = 6454, desc: "ArtNet UDP port", long: "port", short: 'p';
            opt web_port: u16 = 8080, desc: "Port for the web UI", long: "web-port", short: 'w';
        }
        .parse_or_exit();

        // Load config or start with empty state
        let mut emulator_state = if let Some(config_path) = &args.config_path {
            match std::fs::read_to_string(config_path) {
                Ok(json) => match serde_json::from_str::<config::Config>(&json) {
                    Ok(cfg) => {
                        if let Err(e) = cfg.validate() {
                            eprintln!("Config validation error: {e}");
                            std::process::exit(1);
                        }
                        let mut state = cfg.to_emulator_state();
                        state.config_path = Some(config_path.clone());
                        println!("Loaded config '{}': {} universes", config_path, state.universes.len());
                        state
                    }
                    Err(e) => {
                        eprintln!("Error parsing config '{config_path}': {e}");
                        std::process::exit(1);
                    }
                },
                Err(e) => {
                    eprintln!("Error reading config '{config_path}': {e}");
                    std::process::exit(1);
                }
            }
        } else {
            println!("No config file — running in auto-discovery mode");
            state::EmulatorState::empty()
        };

        let shared_state = Arc::new(RwLock::new(emulator_state));
        let shared_log = SharedLog::new(500);
        let state_version = StateVersionNotifier::new();

        let udp_port = args.port;
        let web_port = args.web_port;

        println!("Starting ArtNet Emulator — UDP port: {udp_port}, Web port: {web_port}");

        if std::env::var("PORT").is_err() {
            unsafe { std::env::set_var("PORT", web_port.to_string()) };
        }
        if std::env::var("IP").is_err() {
            unsafe { std::env::set_var("IP", "0.0.0.0") };
        }

        dioxus::serve(move || {
            let shared_state = shared_state.clone();
            let shared_log = shared_log.clone();
            let state_version = state_version.clone();

            async move {
                let bind_addr = format!("0.0.0.0:{udp_port}");
                udp::server::start_server(
                    &bind_addr,
                    shared_state.clone(),
                    shared_log.clone(),
                    state_version.clone(),
                )
                .await;

                let cfg = ServeConfig::new();
                let router = dioxus::server::axum::Router::new()
                    .serve_dioxus_application(cfg, App)
                    .layer(dioxus::server::axum::Extension(shared_state))
                    .layer(dioxus::server::axum::Extension(shared_log))
                    .layer(dioxus::server::axum::Extension(state_version));
                Ok(router)
            }
        });
    }

    #[cfg(not(feature = "server"))]
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    rsx! {
        document::Stylesheet { href: CSS }
        div { class: "min-h-screen bg-gray-900 text-gray-100 p-4",
            h1 { class: "text-2xl font-bold mb-4", "ArtNet DMX Emulator" }
            p { "Listening for ArtNet packets..." }
        }
    }
}
```

**Step 2: Verify it compiles**

Run: `dx build`
Expected: Builds successfully

**Step 3: Commit**

```
git add artnet_emulator/src/main.rs
git commit -m "Wire up main.rs with state, config, log, and UDP server"
```

---

### Task 6: UI snapshot and server functions

Create the snapshot types that serialize state for the browser, and the server functions for fetching and long-polling.

**Files:**
- Create: `artnet_emulator/src/ui/snapshot.rs`
- Modify: `artnet_emulator/src/ui/mod.rs`
- Modify: `artnet_emulator/src/main.rs` (add server functions and update App component)

**Step 1: Create ui/snapshot.rs**

```rust
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

use crate::state::ChannelType;
use crate::udp::log::LogEntry;

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
```

**Step 2: Update ui/mod.rs**

```rust
pub mod snapshot;
```

**Step 3: Add server functions and update App in main.rs**

Add these server functions below the `App` component in main.rs. Also update the `App` component to use long-polling:

```rust
#[server]
async fn get_snapshot() -> Result<ui::snapshot::EmulatorSnapshot, ServerFnError> {
    use dioxus::fullstack::FullstackContext;
    use dioxus::server::axum::Extension;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    let Extension(state): Extension<Arc<RwLock<state::EmulatorState>>> =
        FullstackContext::extract().await?;
    let s = state.read().await;
    Ok(ui::snapshot::build_snapshot(&s))
}

#[server]
async fn wait_for_update(last_version: u64) -> Result<(u64, ui::snapshot::EmulatorSnapshot), ServerFnError> {
    use dioxus::fullstack::FullstackContext;
    use dioxus::server::axum::Extension;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    let Extension(version_notifier): Extension<state::StateVersionNotifier> =
        FullstackContext::extract().await?;
    let Extension(state): Extension<Arc<RwLock<state::EmulatorState>>> =
        FullstackContext::extract().await?;

    let mut rx = version_notifier.subscribe();
    loop {
        if *rx.borrow() != last_version {
            break;
        }
        if rx.changed().await.is_err() {
            break;
        }
    }
    let version = *rx.borrow();

    let s = state.read().await;
    Ok((version, ui::snapshot::build_snapshot(&s)))
}

#[server]
async fn get_log_entries(last_version: u64) -> Result<(u64, std::collections::VecDeque<udp::log::LogEntry>), ServerFnError> {
    use dioxus::fullstack::FullstackContext;
    use dioxus::server::axum::Extension;

    let Extension(shared_log): Extension<udp::log::SharedLog> =
        FullstackContext::extract().await?;
    Ok(shared_log.wait_and_get(last_version).await)
}
```

Update the `App` component:

```rust
#[component]
fn App() -> Element {
    let mut snapshot = use_signal(|| None::<Result<ui::snapshot::EmulatorSnapshot, ServerFnError>>);

    use_future(move || async move {
        let result = get_snapshot().await;
        snapshot.set(Some(result));

        let mut last_version = 0u64;
        loop {
            match wait_for_update(last_version).await {
                Ok((version, snap)) => {
                    last_version = version;
                    snapshot.set(Some(Ok(snap)));
                }
                Err(_) => {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    if let Ok(snap) = get_snapshot().await {
                        snapshot.set(Some(Ok(snap)));
                    }
                }
            }
        }
    });

    match snapshot() {
        Some(Ok(snap)) => {
            let config_label = snap.config_path.as_deref().unwrap_or("Auto-discovery mode");
            let universe_count = snap.universes.len();
            rsx! {
                document::Stylesheet { href: CSS }
                div { class: "min-h-screen bg-gray-900 text-gray-100 p-4",
                    h1 { class: "text-2xl font-bold mb-4", "ArtNet DMX Emulator" }
                    p { class: "text-gray-500 text-sm mb-4", "{config_label} — {universe_count} universes" }

                    if snap.universes.is_empty() {
                        p { class: "text-gray-400 italic", "Waiting for ArtNet packets..." }
                    }

                    for universe in &snap.universes {
                        div { class: "mb-6 bg-gray-800 rounded-lg p-4",
                            h2 { class: "text-lg font-semibold mb-2",
                                "{universe.label}"
                                if let Some(desc) = &universe.description {
                                    span { class: "text-gray-400 text-sm ml-2", "({desc})" }
                                }
                                span { class: "text-gray-500 text-sm ml-2", "— {universe.packet_count} packets" }
                            }

                            // Light view
                            h3 { class: "text-sm font-semibold text-gray-400 mb-2", "Lights" }
                            div { class: "grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-6 gap-2 mb-4",
                                for light in &universe.lights {
                                    {render_light(light)}
                                }
                            }

                            // Raw channel view
                            h3 { class: "text-sm font-semibold text-gray-400 mb-2", "Channels" }
                            div { class: "grid grid-cols-16 gap-px text-xs",
                                for (i, &value) in universe.channels.iter().enumerate() {
                                    {render_channel(i + 1, value)}
                                }
                            }
                        }
                    }
                }
            }
        }
        Some(Err(e)) => rsx! {
            document::Stylesheet { href: CSS }
            div { class: "min-h-screen bg-gray-900 text-red-400 p-4",
                h1 { class: "text-2xl font-bold mb-4", "ArtNet DMX Emulator" }
                p { "Error: {e}" }
            }
        },
        None => rsx! {
            document::Stylesheet { href: CSS }
            div { class: "min-h-screen bg-gray-900 text-gray-400 p-4",
                h1 { class: "text-2xl font-bold mb-4", "ArtNet DMX Emulator" }
                p { "Loading..." }
            }
        },
    }
}

fn render_light(light: &ui::snapshot::LightSnapshot) -> Element {
    use ui::snapshot::LightTypeSnapshot;

    match &light.light_type {
        LightTypeSnapshot::Rgb { r, g, b, .. } => {
            let color = format!("rgb({r},{g},{b})");
            rsx! {
                div { class: "bg-gray-700 rounded p-2 text-center",
                    div {
                        class: "w-full h-8 rounded mb-1",
                        style: "background-color: {color}",
                    }
                    span { class: "text-xs", "{light.name}" }
                    span { class: "text-xs text-gray-400 block", "({r},{g},{b})" }
                }
            }
        }
        LightTypeSnapshot::TriWhite { w1, w2, w3, .. } => {
            let avg = ((*w1 as u16 + *w2 as u16 + *w3 as u16) / 3) as u8;
            let color = format!("rgb({avg},{avg},{avg})");
            rsx! {
                div { class: "bg-gray-700 rounded p-2 text-center",
                    div {
                        class: "w-full h-8 rounded mb-1",
                        style: "background-color: {color}",
                    }
                    span { class: "text-xs", "{light.name}" }
                    span { class: "text-xs text-gray-400 block", "({w1},{w2},{w3})" }
                }
            }
        }
        LightTypeSnapshot::Single { value, .. } => {
            let color = format!("rgb({value},{value},{value})");
            rsx! {
                div { class: "bg-gray-700 rounded p-2 text-center",
                    div {
                        class: "w-full h-8 rounded mb-1",
                        style: "background-color: {color}",
                    }
                    span { class: "text-xs", "{light.name}" }
                    span { class: "text-xs text-gray-400 block", "{value}" }
                }
            }
        }
    }
}

fn render_channel(number: usize, value: u8) -> Element {
    let brightness = (value as f32 / 255.0 * 100.0) as u8;
    let bg = if value > 0 {
        format!("rgba(59, 130, 246, {})", value as f32 / 255.0)
    } else {
        "transparent".to_string()
    };

    rsx! {
        div {
            class: "w-full h-6 text-center leading-6 border border-gray-700 text-xs",
            style: "background-color: {bg}",
            title: "Channel {number}: {value}",
            "{value}"
        }
    }
}
```

**Step 4: Verify it compiles**

Run: `dx build`
Expected: Builds successfully

**Step 5: Commit**

```
git add artnet_emulator/src/ui/ artnet_emulator/src/main.rs
git commit -m "Add UI snapshot, server functions, and web interface"
```

---

### Task 7: End-to-end test and polish

Test the full flow: start the emulator, send ArtNet packets, verify UI updates. Add a log panel and fix any issues.

**Files:**
- Modify: `artnet_emulator/src/main.rs` (add log panel)
- Create: `artnet_emulator/tests/integration.rs` (optional integration test)

**Step 1: Manual end-to-end test**

1. Start the emulator:
   ```bash
   cd artnet_emulator && dx serve
   ```

2. Open browser at `http://localhost:8080`

3. Send a test ArtNet packet using a simple Python script or netcat:
   ```bash
   # Python one-liner to send an ArtNet DMX packet
   python3 -c "
   import socket
   s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
   header = b'Art-Net\x00'
   opcode = b'\x00\x50'  # 0x5000 LE
   version = b'\x00\x0e'
   seq_phys = b'\x00\x00'
   sub_uni = bytes([0x00])  # subnet=0, universe=0
   net = bytes([0x00])
   data = bytes([255, 128, 64, 0, 200, 100, 50, 25] * 4)  # 32 channels
   length = len(data).to_bytes(2, 'big')
   packet = header + opcode + version + seq_phys + sub_uni + net + length + data
   s.sendto(packet, ('127.0.0.1', 6454))
   print(f'Sent {len(data)} channels')
   "
   ```

4. Verify the UI shows a new universe with channel values and auto-generated lights

**Step 2: Add log panel to the UI**

Add a log panel below the universes in the App component. Use a separate `use_future` for log long-polling:

```rust
// Add to App component:
let mut log_entries = use_signal(|| Vec::<udp::log::LogEntry>::new());

use_future(move || async move {
    let mut last_version = 0u64;
    loop {
        match get_log_entries(last_version).await {
            Ok((version, entries)) => {
                last_version = version;
                log_entries.set(entries.into_iter().collect());
            }
            Err(_) => {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }
    }
});

// Add to the UI below universes:
div { class: "mt-6 bg-gray-800 rounded-lg p-4",
    h2 { class: "text-lg font-semibold mb-2", "Activity Log" }
    div { class: "max-h-64 overflow-y-auto text-xs font-mono",
        for entry in log_entries().iter().rev() {
            div { class: "text-gray-400 py-px",
                span { class: "text-gray-500 mr-2", "{entry.timestamp}" }
                "{entry.message}"
            }
        }
        if log_entries().is_empty() {
            p { class: "text-gray-500 italic", "No packets received yet" }
        }
    }
}
```

**Step 3: Fix any compilation or runtime issues found during testing**

**Step 4: Commit**

```
git add artnet_emulator/
git commit -m "Add log panel and verify end-to-end flow"
```

---

## File Summary

| File | Action | Purpose |
|------|--------|---------|
| `artnet_emulator/Cargo.toml` | Create | Crate dependencies and features |
| `artnet_emulator/Dioxus.toml` | Create | Dioxus build configuration |
| `artnet_emulator/assets/main.css` | Create | Stylesheet (can be minimal) |
| `artnet_emulator/src/main.rs` | Create | Entry point, CLI, state wiring, App component, server functions |
| `artnet_emulator/src/lib.rs` | Create | Module declarations |
| `artnet_emulator/src/state.rs` | Create | EmulatorState, UniverseState, StateVersionNotifier, default lights |
| `artnet_emulator/src/config.rs` | Create | JSON config parsing |
| `artnet_emulator/src/udp/mod.rs` | Create | UDP module declarations |
| `artnet_emulator/src/udp/server.rs` | Create | ArtNet UDP listener and packet parser |
| `artnet_emulator/src/udp/log.rs` | Create | Shared activity log with long-poll |
| `artnet_emulator/src/ui/mod.rs` | Create | UI module declarations |
| `artnet_emulator/src/ui/snapshot.rs` | Create | Serializable snapshot types for browser |
