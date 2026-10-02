# MQTT DMX Controller

An MQTT-driven DMX lighting controller written in Rust. It bridges MQTT messages to Art-Net DMX protocol, supporting multi-universe configurations, named light groups, parameterized effects, and automatic dimming.

## Table of Contents

- [Architecture Overview](#architecture-overview)
- [Service Lifecycle](#service-lifecycle)
- [MQTT Protocol](#mqtt-protocol)
  - [Subscribed Topics](#subscribed-topics)
  - [Published Topics](#published-topics)
- [Universe Management](#universe-management)
- [Array Management](#array-management)
  - [Light Groups](#light-groups)
  - [Light Channel Syntax](#light-channel-syntax)
  - [Value Expansion (Variables)](#value-expansion-variables)
- [Effect System](#effect-system)
  - [Effect Types](#effect-types)
  - [Default Effects](#default-effects)
  - [Dimming](#dimming)
  - [Effect Execution](#effect-execution)
- [DMX Channel Types](#dmx-channel-types)
- [Art-Net Output](#art-net-output)
- [Commands](#commands)
- [Configuration Examples](#configuration-examples)

---

## Architecture Overview

The application is structured around four asynchronous workers communicating via message channels:

```
                          +-------------------+
                          |   MQTT Broker     |
                          +--------+----------+
                                   |
                     subscribe DMX/#, publish errors
                                   |
                 +-----------------+-----------------+
                 |                                   |
        +--------v---------+              +----------v--------+
        | MQTT Subscriber  |              | MQTT Publisher    |
        | (event handler)  |              | (error reporter)  |
        +--------+---------+              +-------------------+
                 |                                   ^
      routes messages to managers           error messages
                 |                                   |
       +---------+---------+                         |
       |                   |                         |
+------v-------+   +-------v---------+               |
| Array Manager|   | Artnet Manager  +---------------+
| (config,     |   | (universes,     |
|  effects,    |   |  packets,       |
|  values)     |   |  effects exec)  |
+--------------+   +-----------------+
                           |
                    UDP Art-Net packets
                           |
                   +-------v-------+
                   | DMX Controller|
                   | (hardware)    |
                   +---------------+
```

**Workers and their responsibilities:**

| Worker | Channel Type | Role |
|--------|-------------|------|
| MQTT Subscriber | tokio mpsc | Parses incoming MQTT messages, dispatches to managers |
| MQTT Publisher | async_channel (bounded) | Publishes error reports to MQTT broker |
| Array Manager | tokio mpsc | Stores array definitions, effects, values; builds effect runtime trees |
| Artnet Manager | tokio mpsc | Manages universes, executes effects on a 50ms tick, sends Art-Net packets |

Inter-manager communication uses `tokio::sync::oneshot` channels for request/reply semantics. Each command message includes a oneshot sender so the manager can return a result.

---

## Service Lifecycle

### Startup

1. Parse CLI arguments (MQTT broker address)
2. Initialize tracing/logging
3. Create worker channels (artnet, array, mqtt_publisher)
4. Spawn Artnet Manager worker (with 50ms tick timer)
5. Spawn Array Manager worker
6. Spawn MQTT worker (handles connection and reconnection)

### MQTT Session

Each MQTT session:

1. Connects to the broker, with the last will `DMX/Active = "false"` (retained)
2. Subscribes to `DMX/#`
3. Publishes `DMX/Active = "true"` (retained), once it can hear its commands
4. Republishes its retained model from its own state: `DMX/Version`, and `DMX/LastError` if it has
   reported an error since it started — a broker restarted without its retained messages gets them
   again
5. Spawns publisher and subscriber tasks
6. Waits for either task to fail, then shuts down both

If a session fails, the service waits 10 seconds and reconnects automatically. This loop runs indefinitely.

### Shutdown

SIGTERM (systemd's stop) or SIGINT (Ctrl+C) stops the service within 5 s — its tasks aborted, the
config writer left to write what was saved — and then ends the runtime within 1 s more: a thread
stuck in a synchronous call (a write to a stalled disk) is left behind rather than waited for. A stop
during startup ends the startup at once, the logging's start included: it runs on a blocking thread
raced against the stop (within 15 s; past that the bridge runs without a log). The bridge writes
nothing to stdout or stderr itself once it runs — its lifecycle lines go through the log, whose
writers drop lines rather than wait — so a supervisor's pipe that has stopped draining holds up
neither its start nor its stop.

### Saved configuration

The configuration is saved to the storage directory as each config command is applied. The files are
written by a writer task of their own, on a blocking thread, one write at a time, never by the task
that handles commands: a stalled disk holds one write, later saves of a file replace the one still
waiting, and commands go on. A write that takes over 5 s, or fails, is a WARN (`config_save_failed`,
once per episode); a failed write's content is kept and tried again every 5 s until a save succeeds. At startup the files are read within 10 s; past that the bridge starts without them, and
the broker's retained configs restore them when it subscribes.

---

## MQTT Protocol

### Subscribed Topics

The service subscribes to `DMX/#` and handles the following subtopics:

| Topic Pattern | Payload | Action |
|--------------|---------|--------|
| `DMX/Universe/{id}` | JSON `UniverseDefinition` | Add/update universe |
| `DMX/Universe/{id}` | Empty | Remove universe |
| `DMX/Array/{id}` | JSON `DmxArray` | Add/update array |
| `DMX/Array/{id}` | Empty | Remove array |
| `DMX/Effect/{id}` | JSON `EffectNodeDefinition` | Add/update global effect |
| `DMX/Effect/{id}` | Empty | Remove global effect |
| `DMX/Value/{name}` | JSON `ValueDefinition` | Add/update global value |
| `DMX/Value/{name}` | Empty | Remove global value |
| `DMX/Command/On` | JSON `OnOffCommandParameters` | Start "on" effect on array |
| `DMX/Command/Off` | JSON `OnOffCommandParameters` | Start "off" effect on array |
| `DMX/Command/Dim` | JSON `OnOffCommandParameters` | Start "dim" effect on array |
| `DMX/Command/Stop` | JSON `StopCommandParameters` | Stop running effect on array |
| `DMX/Command/Set` | JSON `SetChannelsParameters` | Set raw channel values |

### Published Topics

| Topic | QoS | Retained | Content |
|-------|-----|----------|---------|
| `DMX/Active` | AtLeastOnce | Yes | `"true"` on connect, `"false"` as last will |
| `DMX/Version` | AtLeastOnce | Yes | Service version string |
| `DMX/LastError` | AtLeastOnce | Yes | JSON `{ time, message }` of last error |
| `DMX/Error` | AtLeastOnce | No | JSON `{ time, message }` for each error |

---

## Universe Management

A universe represents a single Art-Net DMX universe with up to 512 channels, addressed to a specific controller.

### Universe Definition (JSON)

```json
{
    "description": "Main stage lights",
    "controller": "10.0.1.228",
    "net": 0,
    "subnet": 0,
    "universe": 0,
    "channels": 306,
    "log": false,
    "disable_send": false
}
```

| Field | Type | Description |
|-------|------|-------------|
| `description` | string | Human-readable name |
| `controller` | IP address | Art-Net node IP |
| `net` | u8 (0-127) | Art-Net net number |
| `subnet` | u8 (0-15) | Art-Net subnet number |
| `universe` | u8 (0-15) | Art-Net universe number |
| `channels` | u16 (1-512) | Number of DMX channels |
| `log` | bool | Enable channel-level logging |
| `disable_send` | bool | Skip sending packets (for testing) |

Multiple universes can share the same controller IP. Controller connections are pooled automatically.

---

## Array Management

An array is a named group of lights on a specific universe, with associated effects and configuration. Arrays are the primary unit of control.

### Array Definition (JSON)

```json
{
    "universe_id": "0",
    "description": "Living room lights",
    "lights": {
        "all": "@center,@frame,@spot",
        "center": "rgb:1,rgb:4",
        "frame": "s:7,s:8",
        "spot": "$2,w:100"
    },
    "on": "on",
    "off": "off",
    "dim": "dim",
    "effects": {
        "on": {
            "type": "fade",
            "lights": "@all",
            "ticks": 20,
            "target": "s(255);rgb(255,255,255);w(255,255,255)"
        },
        "off": {
            "type": "fade",
            "lights": "@all",
            "ticks": 20,
            "target": "s(0);rgb(0,0,0);w(0,0,0)"
        }
    },
    "default_values": {
        "on_ticks": "20"
    }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `universe_id` | string | (required) | Default universe for channel references |
| `description` | string | (required) | Human-readable name |
| `lights` | map | (required) | Named light groups (must include `all`) |
| `on` | string | `"on"` | Effect ID for the On command |
| `off` | string | `"off"` | Effect ID for the Off command |
| `dim` | string | `"dim"` | Effect ID for the Dim command |
| `effects` | map | `{}` | Effect definitions local to this array |
| `default_values` | map | `{}` | Default variable values for this array |

### Light Groups

Light groups define named sets of DMX channels. The `all` group is mandatory and must encompass every channel referenced in other groups.

Groups support three features:

- **Channel references**: Direct channel definitions (`s:1`, `rgb:4`, `w:10`)
- **Group references**: `@group_name` expands another group recursively (max depth: 5)
- **Universe switching**: `$universe_id` changes the target universe for subsequent entries

### Light Channel Syntax

Channels are specified as comma-separated entries within a light group string:

| Syntax | Description | Example |
|--------|-------------|---------|
| `s:N` | Single channel at address N | `s:7` |
| `rgb:N` | RGB using 3 consecutive channels starting at N | `rgb:1` (channels 1, 2, 3) |
| `rgb:R/G/B` | RGB at specific addresses | `rgb:1/3/5` |
| `w:N` | TriWhite using 3 consecutive channels starting at N | `w:10` (channels 10, 11, 12) |
| `w:W1/W2/W3` | TriWhite at specific addresses | `w:10/12/14` |
| `@name` | Reference another light group by name | `@center` |
| `$id` | Switch subsequent channels to universe `id` | `$2` |

**Example expansion:**

```json
{
    "universe_id": "0",
    "lights": {
        "all": "@center,@frame,@spot",
        "center": "rgb:1,rgb:4",
        "frame": "s:7",
        "spot": "$2,w:100"
    }
}
```

Expanding `@all` produces:
- Universe `0`: RGB at (1,2,3), RGB at (4,5,6), Single at 7
- Universe `2`: TriWhite at (100,101,102)

### Array Validation

When an array is added, the system validates:
- All `@` references resolve to existing groups
- No circular references (max recursion depth: 5)
- Channel types are consistent (a channel can't be used as both Single and RGB Red)
- All channels in non-`all` groups exist in the `all` group

### Value Expansion (Variables)

Effect parameters support variable substitution using backtick syntax:

| Syntax | Description |
|--------|-------------|
| `` `var_name` `` | Replaced with the variable's value |
| `` `var_name=default` `` | Uses default if variable is not set |

**Lookup precedence:**

1. Array-specific values (set via `DMX/Command/On` with `values` field, or `InitializeArrayValues`)
2. Global values (set via `DMX/Value/{name}`)
3. Default value from the expression
4. Error if none found

**Example:**

```json
{
    "type": "fade",
    "ticks": "`on_ticks=10`",
    "target": "`target=s(255);rgb(255,255,255);w(255,255,255)`"
}
```

If `on_ticks` is set to `"20"` for this array, ticks becomes 20. Otherwise it defaults to 10.

---

## Effect System

Effects define how light values change over time. They form a tree structure that is evaluated on every tick (50ms).

### Effect Types

#### Fade

Smoothly interpolates channel values from current to target over a number of ticks.

```json
{
    "type": "fade",
    "lights": "@all",
    "ticks": 20,
    "target": "s(255);rgb(255,255,255);w(255,255,255)",
    "no_dimming": false
}
```

| Field | Type | Description |
|-------|------|-------------|
| `lights` | string | Light group expression (supports `@`, `$`, channel syntax) |
| `ticks` | number or variable | Duration in ticks (1 tick = 50ms) |
| `target` | string or variable | Target values for each channel type |
| `no_dimming` | bool (default: false) | If true, ignore dimming amount |

**Target value syntax:** Semicolon-separated type/value pairs:
- `s(N)` - Single channel target (0-255)
- `rgb(R,G,B)` - RGB target (each 0-255)
- `w(W1,W2,W3)` - TriWhite target (each 0-255)

Only channel types present in the target are faded. If the target specifies `s(255)` but the lights include RGB channels, those RGB channels are left unchanged.

**Interpolation algorithm:** Uses a Bresenham-like integer algorithm for smooth sub-step distribution. The delta per tick is `(target - current) / ticks`, with the remainder distributed evenly across ticks using fraction tracking.

#### Delay

Pauses execution for a number of ticks. Useful in sequences.

```json
{
    "type": "delay",
    "ticks": 10
}
```

#### Sequence

Executes child effects one after another. The next effect starts when the previous one completes.

```json
{
    "type": "sequence",
    "nodes": [
        { "type": "fade", "lights": "@center", "ticks": 10, "target": "rgb(255,0,0)" },
        { "type": "delay", "ticks": 20 },
        { "type": "fade", "lights": "@center", "ticks": 10, "target": "rgb(0,0,0)" }
    ]
}
```

#### Parallel

Executes all child effects simultaneously. Completes when all children are done.

```json
{
    "type": "parallel",
    "nodes": [
        { "type": "fade", "lights": "@center", "ticks": 10, "target": "rgb(255,0,0)" },
        { "type": "fade", "lights": "@frame", "ticks": 20, "target": "s(128)" }
    ]
}
```

### Default Effects

If an array does not define its own `on`, `off`, or `dim` effects, built-in defaults are used:

| Effect | Default Definition |
|--------|-------------------|
| `on` | Fade `@all` to full brightness over `` `on_ticks=10` `` ticks |
| `off` | Fade `@all` to zero over `` `off_ticks=10` `` ticks |
| `dim` | Fade `@all` to full brightness over `` `dim_ticks=10` `` ticks (with dimming applied) |

Default target values:
- Single: 255 (on/dim) or 0 (off)
- RGB: (255,255,255) or (0,0,0)
- TriWhite: (255,255,255) or (0,0,0)

These defaults use variable syntax, so arrays can customize timing via array-specific or global values.

### Dimming

Dimming scales target values by a factor of `dimming_amount / 1000`:

| dimming_amount | Scale | Example: target s(200) |
|---------------|-------|----------------------|
| 1000 (max) | 100% | s(200) |
| 800 | 80% | s(160) |
| 500 | 50% | s(100) |
| 100 | 10% | s(20) |
| 0 | 0% | s(0) |

Dimming is applied at effect creation time, not during execution. Effects with `no_dimming: true` always use full brightness (dimming_amount = 1000).

### Effect Execution

1. A command (On/Off/Dim) arrives via MQTT
2. The Array Manager resolves the effect definition and creates a runtime tree
3. The runtime tree (implementing `EffectNodeRuntime`) is sent to the Artnet Manager
4. The Artnet Manager stores it as an active effect, keyed by the array ID
5. Every 50ms, `tick()` is called on each active effect
6. Effects that report `is_done() == true` are removed
7. Starting a new effect on the same array replaces the previous one

---

## DMX Channel Types

The system supports three channel types:

### Single

A single DMX channel controlling one parameter (typically a dimmer).

- **Definition:** `s:N` where N is the channel address (0-based)
- **Value:** 0-255
- **Target syntax:** `s(N)`

### RGB

Three DMX channels for Red, Green, Blue color mixing.

- **Definition:** `rgb:N` (consecutive: N, N+1, N+2) or `rgb:R/G/B` (specific addresses)
- **Value:** Three bytes (R, G, B), each 0-255
- **Target syntax:** `rgb(R,G,B)`

### TriWhite

Three DMX channels for three-color warm white mixing.

- **Definition:** `w:N` (consecutive: N, N+1, N+2) or `w:W1/W2/W3` (specific addresses)
- **Value:** Three bytes (W1, W2, W3), each 0-255
- **Target syntax:** `w(W1,W2,W3)`

---

## Art-Net Output

The service sends standard Art-Net DMX512 packets over UDP (port 0x1936):

- Packet structure: 18-byte header + channel data (up to 512 bytes, padded to even count)
- Sequence number incremented per packet per universe
- Modified universes are sent on each 50ms tick
- Unmodified universes are re-sent every 4 seconds (80 ticks) to maintain controller state
- Controllers are shared across universes on the same IP (connection pooled)

---

## Commands

### On / Off / Dim

```json
{
    "array_id": "living_room",
    "effect_id": "custom_on",
    "dimming_amount": 800,
    "values": {
        "on_ticks": "30",
        "target": "s(200);rgb(200,180,150)"
    }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `array_id` | string | Yes | Target array |
| `effect_id` | string | No | Override default effect for this usage |
| `dimming_amount` | number | No | 0-1000, defaults to 1000 (full) |
| `values` | map | No | Array-specific variable values |

Publish to `DMX/Command/On`, `DMX/Command/Off`, or `DMX/Command/Dim`.

### Stop

```json
{
    "array_id": "living_room"
}
```

Publish to `DMX/Command/Stop`. Removes the active effect on the specified array. Channels remain at their last value.

### Set

```json
{
    "universe_id": "0",
    "channels": "s:5,rgb:10",
    "target": "s(128);rgb(255,0,0)",
    "dimming_amount": 1000
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `universe_id` | string | Yes | Target universe |
| `channels` | string | Yes | Channel definitions (comma-separated) |
| `target` | string | Yes | Target value string |
| `dimming_amount` | number | No | Optional dimming |

Publish to `DMX/Command/Set`. Sets channels directly without an effect (immediate, no fade).

---

## Configuration Examples

### Simple Dimmer Setup

Universe:
```json
// Publish to DMX/Universe/0
{
    "description": "Stage dimmers",
    "controller": "10.0.1.100",
    "net": 0, "subnet": 0, "universe": 0,
    "channels": 24
}
```

Array:
```json
// Publish to DMX/Array/stage
{
    "universe_id": "0",
    "description": "Stage wash",
    "lights": {
        "all": "s:0,s:1,s:2,s:3"
    }
}
```

Turn on at 80% brightness:
```json
// Publish to DMX/Command/On
{ "array_id": "stage", "dimming_amount": 800 }
```

### RGB Light Setup with Custom Effects

```json
// Publish to DMX/Array/bar
{
    "universe_id": "0",
    "description": "Bar LED strips",
    "lights": {
        "all": "@left,@right",
        "left": "rgb:1,rgb:4",
        "right": "rgb:7,rgb:10"
    },
    "effects": {
        "on": {
            "type": "sequence",
            "nodes": [
                {
                    "type": "parallel",
                    "nodes": [
                        { "type": "fade", "lights": "@left", "ticks": 10, "target": "rgb(255,200,100)" },
                        { "type": "fade", "lights": "@right", "ticks": 20, "target": "rgb(255,200,100)" }
                    ]
                }
            ]
        },
        "off": {
            "type": "fade",
            "lights": "@all",
            "ticks": 40,
            "target": "rgb(0,0,0)"
        }
    }
}
```

### Multi-Universe Setup

```json
// Publish to DMX/Array/whole_room
{
    "universe_id": "0",
    "description": "All room lights",
    "lights": {
        "all": "@ceiling,@wall",
        "ceiling": "s:0,s:1,s:2",
        "wall": "$1,rgb:0,rgb:3,rgb:6"
    }
}
```

The `ceiling` group uses channels on universe `0`, while `wall` switches to universe `1` for its RGB fixtures.

### Parameterized Effects with Variables

Global value:
```json
// Publish to DMX/Value/fade_speed
{ "value": "15" }
```

Array using variables:
```json
// Publish to DMX/Array/kitchen
{
    "universe_id": "0",
    "description": "Kitchen lights",
    "lights": { "all": "w:0" },
    "effects": {
        "on": {
            "type": "fade",
            "lights": "@all",
            "ticks": "`fade_speed=10`",
            "target": "`kitchen_color=w(255,200,180)`"
        }
    }
}
```

Sending a command with runtime values:
```json
// Publish to DMX/Command/On
{
    "array_id": "kitchen",
    "values": {
        "kitchen_color": "w(255,255,255)",
        "fade_speed": "30"
    }
}
```
