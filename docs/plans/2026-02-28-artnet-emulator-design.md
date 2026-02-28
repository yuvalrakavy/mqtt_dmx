# ArtNet Emulator Design

## Goal

A standalone ArtNet emulator for testing the mqtt_dmx bridge without real DMX hardware. Receives ArtNet UDP packets, tracks channel state, and displays it in a web UI. Follows the same architectural pattern as the HDL and Lutron emulators.

## Architecture

A Dioxus fullstack app (`artnet_emulator/` crate) with two subsystems:

1. **UDP server** on port 6454 — receives ArtNet DMX Output packets (opcode 0x5000), parses net/subnet/universe/channel data, updates state
2. **Dioxus web UI** — real-time universe/channel visualization with two views

Shared state via `Arc<RwLock<EmulatorState>>` between UDP listener and web UI, with `StateVersionNotifier` for long-poll updates (same pattern as HDL/Lutron emulators).

## Data Flow

```
mqtt_dmx bridge  --[ArtNet UDP packets]--> UDP listener
                                              |
                                              v
                                         Update EmulatorState
                                              |
                                              v
                                         Dioxus web UI (long-poll)
```

This is a **receive-only** emulator. It does not send ArtNet packets back — it only observes and displays what mqtt_dmx sends.

## State Model

```rust
EmulatorState {
    universes: HashMap<UniverseKey, UniverseState>,
}

// UniverseKey = (net: u8, subnet: u8, universe: u8)
UniverseKey(u8, u8, u8)

UniverseState {
    key: UniverseKey,
    description: Option<String>,
    channel_count: u16,             // from config or from packet length
    channels: [u8; 512],            // raw DMX values
    lights: Vec<LightDefinition>,   // logical groupings
    last_update: Instant,
    packet_count: u64,
}

LightDefinition {
    name: String,
    channel_def: ChannelDefinition, // Single(ch) / Rgb(r,g,b) / TriWhite(w1,w2,w3)
}
```

## Auto-Discovery

When an ArtNet packet arrives for an unknown (net, subnet, universe), the emulator creates a new `UniverseState` dynamically:
- Channel count inferred from packet data length
- Lights auto-generated with a mix of types: cycling through RGB, tri-white, and single channels to cover all code paths

## Default Light Generation

When a universe has no configured lights, auto-generate a mix:
- Cycle through: RGB (3 channels), tri-white (3 channels), single (1 channel)
- Name them: "RGB 1", "White 1", "Single 1", "RGB 2", etc.
- Continue until all channels in the universe are assigned

Example for 32 channels:
- Channels 1-3: "RGB 1" (rgb:1)
- Channels 4-6: "White 1" (w:4)
- Channel 7: "Single 1" (s:7)
- Channels 8-10: "RGB 2" (rgb:8)
- Channels 11-13: "White 2" (w:11)
- Channel 14: "Single 2" (s:14)
- ...

## Configuration

Optional JSON config file. No config = pure auto-discovery mode.

```json
{
  "port": 6454,
  "web_port": 8080,
  "universes": [
    {
      "net": 0,
      "subnet": 0,
      "universe": 0,
      "channels": 32,
      "description": "Main",
      "lights": [
        { "name": "Kitchen Spots", "channels": "rgb:1" },
        { "name": "Hallway", "channels": "s:4" },
        { "name": "Living Room", "channels": "w:5" }
      ]
    }
  ]
}
```

All fields optional. `port` defaults to 6454, `web_port` defaults to 8080.

## Web UI

- **Universe list** — collapsible panels, one per universe, showing description and packet stats
- **Raw channel view** — grid of channel values with colored intensity bars (0-255)
- **Light view** — named lights with color visualization:
  - RGB: color swatch showing the actual RGB color
  - Tri-white: brightness bar
  - Single: brightness bar
- **Log panel** — recent ArtNet packets (timestamp, universe address, channel count)
- **Toggle** between raw channel and light views per universe

## CLI

```bash
artnet_emulator [--config <file>] [--port <udp_port>] [--web-port <http_port>]
```

## ArtNet Packet Parsing

ArtNet DMX Output packet format (opcode 0x5000):
```
Offset  Size  Field
0       8     "Art-Net\0" header
8       2     OpCode (0x0050 little-endian)
10      2     Protocol version (0x000e)
12      1     Sequence
13      1     Physical
14      1     SubUniverse (subnet << 4 | universe)
15      1     Net
16      2     Data length (high, low)
18      N     DMX channel data (1-512 bytes)
```

Extract: `net` from byte 15, `subnet` from high nibble of byte 14, `universe` from low nibble of byte 14. Copy channel data from offset 18.

## Error Handling

- Invalid ArtNet packets (wrong header, bad opcode): log and discard
- Config file errors: log and start in auto-discovery-only mode
- UDP bind failures: exit with error message

## Project Structure

```
artnet_emulator/
  Cargo.toml
  Dioxus.toml
  src/
    main.rs          # CLI, Dioxus launch, UDP server spawn
    lib.rs           # Module declarations
    state.rs         # EmulatorState, UniverseState, StateVersionNotifier
    config.rs        # Config deserialization, default light generation
    udp/
      mod.rs
      server.rs      # UDP listener, ArtNet packet parsing
      log.rs         # Activity log with long-poll support
    ui/
      mod.rs
      app.rs         # Root component, layout
      universe.rs    # Universe panel, raw/light view toggle
      channel_grid.rs # Raw channel value grid
      light_view.rs  # Logical light visualization
      log_panel.rs   # Packet log display
```
