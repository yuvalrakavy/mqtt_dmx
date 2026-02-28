# Local Configuration Persistence Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Persist Universe, Array, Effect, and Value configurations to local JSON files so the service can restore state when MQTT retained messages are missing.

**Architecture:** A new `persistence` module provides load/save functions for each entity type. On startup, `Service::start()` loads persisted state and replays it through existing manager channels. The MQTT subscriber calls persistence saves after each successful config change. Persistence errors are logged but never fail the operation.

**Tech Stack:** serde_json (already a dependency), std::fs for file I/O, std::path::PathBuf for cross-platform paths.

---

### Task 1: Add Serialize to data types

The persistence module needs to serialize configurations to JSON. Currently `DmxArray`, `EffectNodeDefinition`, and their nested types only derive `Deserialize`.

**Files:**
- Modify: `mqtt_dmx/src/defs.rs`

**Step 1: Add Serialize import and derive to all configuration types**

Add `Serialize` to the `use serde` import at line 1, then add `Serialize` derive to each struct/enum that will be persisted:

```rust
// Line 1: change import
use serde::{Deserialize, Serialize};

// Line 8: UniverseDefinition - add Serialize
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct UniverseDefinition {

// Line 25: ValueDefinition - add Serialize
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ValueDefinition {

// Line 35: DmxArray - add Serialize
#[derive(Debug, Deserialize, Serialize)]
#[allow(dead_code)]
pub struct DmxArray {

// Line 66: TargetValue - add Serialize
#[derive(Debug, Deserialize, Serialize, Default)]
pub struct TargetValue {

// Line 73: NumberOrVariable - add Serialize
#[derive(Deserialize, Serialize, Debug)]
#[serde(untagged)]
pub enum NumberOrVariable {

// Line 101: EffectNodeDefinition - add Serialize
#[derive(Deserialize, Serialize, Debug)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum EffectNodeDefinition {

// Line 111: SequenceEffectNodeDefinition - add Serialize
#[derive(Deserialize, Serialize, Debug)]
pub struct SequenceEffectNodeDefinition {

// Line 116: ParallelEffectNodeDefinition - add Serialize
#[derive(Deserialize, Serialize, Debug)]
pub struct ParallelEffectNodeDefinition {

// Line 121: DelayEffectNodeDefinition - add Serialize
#[derive(Deserialize, Serialize, Debug)]
pub struct DelayEffectNodeDefinition {

// Line 126: FadeEffectNodeDefinition - add Serialize
#[derive(Deserialize, Serialize, Debug)]
pub struct FadeEffectNodeDefinition {
```

**Step 2: Verify it compiles**

Run: `cargo clippy --tests`
Expected: No errors or warnings

**Step 3: Commit**

```
git add mqtt_dmx/src/defs.rs
git commit -m "Add Serialize derive to configuration types for persistence"
```

---

### Task 2: Create the persistence module

**Files:**
- Create: `mqtt_dmx/src/persistence.rs`
- Modify: `mqtt_dmx/src/main.rs` (add `mod persistence;`)

**Step 1: Write the persistence module**

The module provides:
- `Persistence` struct holding the storage `PathBuf`
- `load_*` methods that read JSON files and return `HashMap`s (or empty maps if file missing)
- `save_*` methods that write JSON files atomically
- All methods are sync (files are small, I/O is fast)

```rust
use log::{error, info};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::defs::{DmxArray, EffectNodeDefinition, UniverseDefinition};

pub struct Persistence {
    storage_path: PathBuf,
}

impl Persistence {
    pub fn new(storage_path: PathBuf) -> Self {
        Self { storage_path }
    }

    pub fn ensure_directory(&self) -> Result<(), std::io::Error> {
        fs::create_dir_all(&self.storage_path)
    }

    fn file_path(&self, filename: &str) -> PathBuf {
        self.storage_path.join(filename)
    }

    fn load_file<T: for<'de> Deserialize<'de>>(&self, filename: &str) -> HashMap<Arc<str>, T> {
        let path = self.file_path(filename);
        match fs::read_to_string(&path) {
            Ok(contents) => match serde_json::from_str(&contents) {
                Ok(data) => {
                    info!("Loaded {} from {}", filename, path.display());
                    data
                }
                Err(e) => {
                    error!("Failed to parse {}: {}", path.display(), e);
                    HashMap::new()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                info!("No persisted {} found at {}", filename, path.display());
                HashMap::new()
            }
            Err(e) => {
                error!("Failed to read {}: {}", path.display(), e);
                HashMap::new()
            }
        }
    }

    fn save_file<T: Serialize>(&self, filename: &str, data: &HashMap<Arc<str>, T>) {
        let path = self.file_path(filename);
        match serde_json::to_string_pretty(data) {
            Ok(json) => {
                if let Err(e) = fs::write(&path, json) {
                    error!("Failed to write {}: {}", path.display(), e);
                }
            }
            Err(e) => {
                error!("Failed to serialize {}: {}", path.display(), e);
            }
        }
    }

    // --- Universes ---

    pub fn load_universes(&self) -> HashMap<Arc<str>, UniverseDefinition> {
        self.load_file("universes.json")
    }

    pub fn save_universes(&self, universes: &HashMap<Arc<str>, UniverseDefinition>) {
        self.save_file("universes.json", universes);
    }

    // --- Arrays ---

    pub fn load_arrays(&self) -> HashMap<Arc<str>, DmxArray> {
        self.load_file("arrays.json")
    }

    pub fn save_arrays(&self, arrays: &HashMap<Arc<str>, DmxArray>) {
        self.save_file("arrays.json", arrays);
    }

    // --- Effects ---

    pub fn load_effects(&self) -> HashMap<Arc<str>, EffectNodeDefinition> {
        self.load_file("effects.json")
    }

    pub fn save_effects(&self, effects: &HashMap<Arc<str>, EffectNodeDefinition>) {
        self.save_file("effects.json", effects);
    }

    // --- Values ---

    pub fn load_values(&self) -> HashMap<Arc<str>, String> {
        self.load_file("values.json")
    }

    pub fn save_values(&self, values: &HashMap<Arc<str>, String>) {
        self.save_file("values.json", values);
    }
}
```

**Step 2: Register the module in main.rs**

Add `mod persistence;` after the existing module declarations in `mqtt_dmx/src/main.rs` (after line 10):

```rust
mod persistence;
```

**Step 3: Verify it compiles**

Run: `cargo clippy --tests`
Expected: No errors (warning about unused `Persistence` is OK for now)

**Step 4: Commit**

```
git add mqtt_dmx/src/persistence.rs mqtt_dmx/src/main.rs
git commit -m "Add persistence module with load/save for all config types"
```

---

### Task 3: Add storage path CLI argument and env var

**Files:**
- Modify: `mqtt_dmx/src/main.rs` (add `--storage` param)
- Modify: `mqtt_dmx/src/service.rs:21-23` (`ServiceConfig` — add `storage_path` field)

**Step 1: Add storage_path to ServiceConfig**

In `mqtt_dmx/src/service.rs`, add the field:

```rust
use std::path::PathBuf;
// ...
pub struct ServiceConfig {
    pub mqtt_broker_address: String,
    pub storage_path: PathBuf,
}
```

**Step 2: Add CLI param and env var resolution in main.rs**

In `mqtt_dmx/src/main.rs`, add the CLI param and resolve with env var fallback:

```rust
use std::path::PathBuf;
// ...
let (args, _) = opts! {
    synopsis "MQTT DMX Controller";
    param mqtt:String, desc: "MQTT broker to connect";
    opt storage:Option<String>, desc: "Path to config storage directory";
}.parse_or_exit();

// Resolve storage path: CLI > env var > default
let storage_path = PathBuf::from(
    args.storage
        .or_else(|| std::env::var("MQTT_DMX_STORAGE_PATH").ok())
        .unwrap_or_else(|| "dmx_config".to_string()),
);

let config = ServiceConfig {
    mqtt_broker_address: args.mqtt,
    storage_path,
};
```

**Step 3: Verify it compiles**

Run: `cargo clippy --tests`
Expected: No errors

**Step 4: Commit**

```
git add mqtt_dmx/src/main.rs mqtt_dmx/src/service.rs
git commit -m "Add --storage CLI argument with env var fallback"
```

---

### Task 4: Integrate persistence into MQTT subscriber

The subscriber needs a `Persistence` handle to save after each successful config change. It also needs to track in-memory copies of the persisted maps so it can update and save them.

**Files:**
- Modify: `mqtt_dmx/src/mqtt_subscriber.rs`
- Modify: `mqtt_dmx/src/service.rs` (pass persistence to subscriber)

**Step 1: Add persistence to MqttSubscriber and session()**

In `mqtt_dmx/src/mqtt_subscriber.rs`:

Add import at the top:
```rust
use std::sync::Arc as StdArc;
use std::collections::HashMap;
use crate::persistence::Persistence;
```

Change the `MqttSubscriber` struct to hold persistence state:

```rust
struct MqttSubscriber {
    to_artnet_tx: Sender<messages::ToArtnetManagerMessage>,
    to_array_tx: Sender<messages::ToArrayManagerMessage>,
    to_mqtt_publisher_tx: async_channel::Sender<messages::ToMqttPublisherMessage>,
    persistence: StdArc<Persistence>,
    universes: HashMap<Arc<str>, defs::UniverseDefinition>,
    arrays: HashMap<Arc<str>, defs::DmxArray>,
    effects: HashMap<Arc<str>, defs::EffectNodeDefinition>,
    values: HashMap<Arc<str>, String>,
}
```

Note: Uses `StdArc` alias to avoid conflict with `Arc<str>` which is `std::sync::Arc<str>`. Alternatively, use the full path `std::sync::Arc<Persistence>`.

Actually, simpler approach — since `Arc` is already imported as `std::sync::Arc`, just use it directly:

```rust
use crate::persistence::Persistence;

struct MqttSubscriber {
    to_artnet_tx: Sender<messages::ToArtnetManagerMessage>,
    to_array_tx: Sender<messages::ToArrayManagerMessage>,
    to_mqtt_publisher_tx: async_channel::Sender<messages::ToMqttPublisherMessage>,
    persistence: Arc<Persistence>,
    universes: HashMap<Arc<str>, defs::UniverseDefinition>,
    arrays: HashMap<Arc<str>, defs::DmxArray>,
    effects: HashMap<Arc<str>, defs::EffectNodeDefinition>,
    values: HashMap<Arc<str>, String>,
}
```

Add `std::collections::HashMap` to imports.

Update `session()` signature to accept persistence:

```rust
pub async fn session(
    mut event_loop: EventLoop,
    to_artnet_tx: Sender<messages::ToArtnetManagerMessage>,
    to_array_tx: Sender<messages::ToArrayManagerMessage>,
    to_mqtt_publisher_tx: async_channel::Sender<messages::ToMqttPublisherMessage>,
    persistence: Arc<Persistence>,
) -> Result<(), Report<MqttError>> {
```

Initialize MqttSubscriber with empty maps (they'll be populated as MQTT messages arrive):

```rust
let mqtt_subscriber = MqttSubscriber {
    to_artnet_tx,
    to_array_tx,
    to_mqtt_publisher_tx,
    persistence,
    universes: HashMap::new(),
    arrays: HashMap::new(),
    effects: HashMap::new(),
    values: HashMap::new(),
};
```

**Important**: Change `&self` to `&mut self` on all handler methods since they now mutate the persistence maps. Also change the `handle_message` call from `mqtt_subscriber.handle_message(...)` to use a mutable reference — change `let mqtt_subscriber` to `let mut mqtt_subscriber`.

**Step 2: Add persistence saves to each handler**

In `handle_universe_message`: after the successful add (line ~174) or remove (line ~156), update the local map and save:

For **add** (after the `send_artnet` + `recv_reply` succeeds, before returning `Ok`):
```rust
self.universes.insert(universe_id, definition);
self.persistence.save_universes(&self.universes);
```

Note: the `definition` was moved into the message. To keep a copy, clone it before sending. Change the add branch to clone the definition:
```rust
Ok(definition) => {
    let (tx_artnet_reply, rx_artnet_reply) =
        oneshot::channel::<Result<(), Report<ArtnetError>>>();

    self.send_artnet(messages::ToArtnetManagerMessage::AddUniverse(
        universe_id.clone(),
        definition.clone(),
        tx_artnet_reply,
    ))
    .await?;

    if let Err(e) = Self::recv_reply(rx_artnet_reply.await)? {
        return Err(e).change_context_lazy(|| {
            MqttError::Context(format!("adding universe {universe_id}"))
        });
    }

    self.universes.insert(universe_id, definition);
    self.persistence.save_universes(&self.universes);
}
```

For **remove** (after successful remove reply):
```rust
self.universes.remove(&universe_id);
self.persistence.save_universes(&self.universes);
```

Note: `universe_id` was moved into the message. Clone it before sending so we can use it for the remove. The remove branch needs `universe_id.clone()` in the message and then use `universe_id` for the map remove.

Apply the same pattern for `handle_array_message`, `handle_effect_message`, and `handle_value_message`:

- **Arrays**: `self.arrays.insert(array_id, definition)` / `self.arrays.remove(&array_id)` + `self.persistence.save_arrays(&self.arrays)`
- **Effects**: `self.effects.insert(effect_id, effect_definition)` / `self.effects.remove(&effect_id)` + `self.persistence.save_effects(&self.effects)`
- **Values**: `self.values.insert(value_name, value_definition.value.to_string())` / `self.values.remove(&value_name)` + `self.persistence.save_values(&self.values)`

Note: For arrays, `DmxArray` needs `Clone` derive. Add `#[derive(Debug, Deserialize, Serialize, Clone)]` to `DmxArray` in `defs.rs`. Same for `EffectNodeDefinition` and its nested types — add `Clone` to all of them:
- `EffectNodeDefinition`
- `SequenceEffectNodeDefinition`
- `ParallelEffectNodeDefinition`
- `DelayEffectNodeDefinition`
- `FadeEffectNodeDefinition`
- `NumberOrVariable`

**Step 3: Update service.rs to pass persistence to subscriber**

In `mqtt_dmx/src/service.rs`, update `mqtt_session` and `mqtt` to accept and pass through persistence:

```rust
async fn mqtt_session(
    broker_address: &str,
    to_artnet_tx: Sender<ToArtnetManagerMessage>,
    to_array_tx: Sender<messages::ToArrayManagerMessage>,
    to_mqtt_publisher_rx: async_channel::Receiver<messages::ToMqttPublisherMessage>,
    to_mqtt_publisher_tx: async_channel::Sender<messages::ToMqttPublisherMessage>,
    persistence: Arc<Persistence>,
) -> Result<(), Report<MqttError>> {
    // ... existing code ...
    mqtt_workers.spawn(async move {
        let e = mqtt_subscriber::session(
            mqtt_event_loop,
            to_artnet_tx,
            to_array_tx,
            to_mqtt_publisher_tx,
            persistence,
        )
        .await;
        info!("MQTT subscriber session ended: {:?}", e)
    });
    // ...
}

async fn mqtt(
    broker_address: &str,
    to_artnet_tx: Sender<ToArtnetManagerMessage>,
    to_array_tx: Sender<messages::ToArrayManagerMessage>,
    to_mqtt_publisher_rx: async_channel::Receiver<messages::ToMqttPublisherMessage>,
    to_mqtt_publisher_tx: async_channel::Sender<messages::ToMqttPublisherMessage>,
    persistence: Arc<Persistence>,
) {
    loop {
        let _ = Self::mqtt_session(
            broker_address,
            to_artnet_tx.clone(),
            to_array_tx.clone(),
            to_mqtt_publisher_rx.clone(),
            to_mqtt_publisher_tx.clone(),
            persistence.clone(),
        )
        .await;
        // ...
    }
}
```

Add imports in service.rs:
```rust
use std::sync::Arc;
use crate::persistence::Persistence;
```

Update the mqtt worker spawn in `Service::start()` to create and pass the `Persistence`:

```rust
let persistence = Arc::new(Persistence::new(self.config.storage_path.clone()));
if let Err(e) = persistence.ensure_directory() {
    error!("Failed to create storage directory: {}", e);
}

// ... in the mqtt spawn:
self.workers.spawn(async move {
    Self::mqtt(
        &broker_address,
        to_artnet_tx,
        to_array_tx,
        to_mqtt_publisher_rx,
        to_mqtt_publisher_tx,
        persistence,
    )
    .await;
});
```

Add `use log::{info, error};` to service.rs imports (currently only `info`).

**Step 4: Verify it compiles**

Run: `cargo clippy --tests`
Expected: No errors

**Step 5: Commit**

```
git add mqtt_dmx/src/mqtt_subscriber.rs mqtt_dmx/src/service.rs mqtt_dmx/src/defs.rs
git commit -m "Integrate persistence saves into MQTT subscriber"
```

---

### Task 5: Load persisted state on startup

**Files:**
- Modify: `mqtt_dmx/src/service.rs` (`Service::start()`)

**Step 1: Add startup loading after spawning workers but before spawning MQTT**

In `Service::start()`, after spawning the artnet and array manager workers, load persisted state and replay it through the channels. This must happen before spawning the MQTT worker so persisted state is loaded first, then MQTT retained messages can overwrite.

Insert this block between the array manager spawn and the MQTT worker spawn:

```rust
// Load persisted state and replay through channels
let persisted_universes = persistence.load_universes();
let persisted_arrays = persistence.load_arrays();
let persisted_effects = persistence.load_effects();
let persisted_values = persistence.load_values();

for (universe_id, definition) in persisted_universes {
    let (tx, rx) = tokio::sync::oneshot::channel();
    if to_artnet_tx
        .send(ToArtnetManagerMessage::AddUniverse(universe_id.clone(), definition, tx))
        .await
        .is_ok()
    {
        if let Ok(Err(e)) = rx.await {
            error!("Failed to restore universe {}: {:?}", universe_id, e);
        }
    }
}

for (array_id, definition) in persisted_arrays {
    let (tx, rx) = tokio::sync::oneshot::channel();
    if to_array_tx
        .send(messages::ToArrayManagerMessage::AddArray(
            array_id.clone(),
            Box::new(definition),
            tx,
        ))
        .await
        .is_ok()
    {
        if let Ok(Err(e)) = rx.await {
            error!("Failed to restore array {}: {:?}", array_id, e);
        }
    }
}

for (effect_id, definition) in persisted_effects {
    let (tx, rx) = tokio::sync::oneshot::channel();
    if to_array_tx
        .send(messages::ToArrayManagerMessage::AddEffect(effect_id.clone(), definition, tx))
        .await
        .is_ok()
    {
        if let Ok(Err(e)) = rx.await {
            error!("Failed to restore effect {}: {:?}", effect_id, e);
        }
    }
}

for (value_name, value) in persisted_values {
    let (tx, rx) = tokio::sync::oneshot::channel();
    if to_array_tx
        .send(messages::ToArrayManagerMessage::AddGlobalValue(
            value_name.clone(),
            Arc::from(value.as_str()),
            tx,
        ))
        .await
        .is_ok()
    {
        if let Ok(Err(e)) = rx.await {
            error!("Failed to restore value {}: {:?}", value_name, e);
        }
    }
}

info!("Persisted configuration loaded from {}", self.config.storage_path.display());
```

Note: `Service::start()` must now be async and uses the channels before they're moved into workers. The `to_artnet_tx` and `to_array_tx` are cloneable (`Sender` is `Clone`), so clone them for the replay and move the originals into the MQTT worker as before.

Update the start method to clone channels for replay:

```rust
let to_artnet_tx_replay = to_artnet_tx.clone();
let to_array_tx_replay = to_array_tx.clone();
```

Use `to_artnet_tx_replay` and `to_array_tx_replay` for the loading loop, keep `to_artnet_tx` and `to_array_tx` for the MQTT worker spawn.

**Step 2: Verify it compiles**

Run: `cargo clippy --tests`
Expected: No errors

**Step 3: Commit**

```
git add mqtt_dmx/src/service.rs
git commit -m "Load persisted configuration on service startup"
```

---

### Task 6: Verify end-to-end and clean up

**Files:**
- All modified files

**Step 1: Run clippy**

Run: `cargo clippy --tests`
Expected: No errors, no warnings

**Step 2: Run tests**

Run: `cargo test`
Expected: Compiles (tests may not execute due to cross-compilation target, but compilation must succeed)

**Step 3: Final commit (if any cleanup needed)**

```
git add -A
git commit -m "Clean up persistence implementation"
```

---

## File Summary

| File | Action | Purpose |
|------|--------|---------|
| `mqtt_dmx/src/defs.rs` | Modify | Add `Serialize` and `Clone` to config types |
| `mqtt_dmx/src/persistence.rs` | Create | New module with load/save functions |
| `mqtt_dmx/src/main.rs` | Modify | Add `mod persistence`, `--storage` CLI arg |
| `mqtt_dmx/src/service.rs` | Modify | Add `storage_path` to config, pass persistence, load on startup |
| `mqtt_dmx/src/mqtt_subscriber.rs` | Modify | Add persistence tracking and saves after config changes |
