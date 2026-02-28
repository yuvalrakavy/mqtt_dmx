# DMX SDL Module Design

## Overview

An SDL module (`dmx.sdl`) for the Store server that provides full configuration management and control of the mqtt_dmx bridge. The module publishes universe, array, effect, and value definitions to MQTT and sends On/Off/Dim/Stop commands. It also subscribes to mqtt_dmx status and error topics.

## Design Decisions

- **Arrays are the BindTarget level** — arrays are mqtt_dmx's primary unit of control
- **Full config management** — the SDL module owns all DMX configuration, publishing to MQTT
- **Manual creation only** — no auto-discovery; the user builds the hierarchy in Store
- **Effects as SDL object tree** — Fade, Delay, Sequence, Parallel modeled as containable classes
- **Channel strings, not objects** — LightGroups use mqtt_dmx's native channel syntax (`rgb:1`, `s:7`, `@group`, `$universe`)
- **Explicit publish/unpublish** — all config objects require manual publish via trigger properties
- **Status feedback** — subscribes to `DMX/Active` and `DMX/Error` for service health

## Object Hierarchy

```
DMX (DeviceObject, UsingMqtt, MqttSubscriber)
  ├─ Universes (container)
  │    └─ Universe [Publish/Unpublish/Status]
  ├─ Arrays (container)
  │    └─ Array (BindTarget, has PowerState, Dimmer, Fade) [Publish/Unpublish/Status]
  │         ├─ LightGroup
  │         ├─ ArrayEffect
  │         │    └─ FadeEffect / DelayEffect / SequenceEffect / ParallelEffect (nested)
  │         └─ ArrayValue
  ├─ Effects (container)
  │    └─ GlobalEffect [Publish/Unpublish/Status]
  │         └─ FadeEffect / DelayEffect / SequenceEffect / ParallelEffect (nested)
  └─ Values (container)
       └─ GlobalValue [Publish/Unpublish/Status]
```

## Classes

### DMX (root)

The module root. One per system.

| Trait | Purpose |
|-------|---------|
| `DeviceObject` | Top-level infrastructure object |
| `UsingMqtt` | Links to an MqttClient |
| `MqttSubscriber` | Subscribes to status topics |

**Properties:**

| Property | Type | Notes |
|----------|------|-------|
| `Active` | `Boolean? managed memoryonly` | Reflects mqtt_dmx service status |
| `LastError` | `String? managed memoryonly` | Last error message from mqtt_dmx |
| `LastErrorTime` | `String? managed memoryonly` | Timestamp of last error |

**MQTT subscriptions:**

| Topic | Handler |
|-------|---------|
| `DMX/Active` | Sets `Active` property |
| `DMX/LastError` | Parses JSON, sets `LastError` and `LastErrorTime` |

**Contains:** Universes, Arrays, Effects, Values (auto-created on `on Created`).

### Container Classes

`Universes`, `Arrays`, `Effects`, `Values` — simple container classes with no logic. They exist for UI organization.

### Universe

Represents a single Art-Net DMX universe.

**Properties:**

| Property | Type | Constraints | Notes |
|----------|------|-------------|-------|
| `Description` | `String` | | Human-readable name |
| `ControllerAddress` | `String` | | Art-Net node IP address |
| `Net` | `Int` | min: 0, max: 127 | Art-Net net number |
| `Subnet` | `Int` | min: 0, max: 15 | Art-Net subnet |
| `UniverseNumber` | `Int` | min: 0, max: 15 | Art-Net universe |
| `Channels` | `Int` | min: 1, max: 512 | Number of DMX channels |
| `Log` | `Boolean?` | | Enable channel-level logging |
| `DisableSend` | `Boolean?` | | Skip sending packets (testing) |
| `Status` | `String? managed memoryonly` | | Modified / Published / Unpublished |
| `Publish` | trigger | | Publishes config to MQTT |
| `Unpublish` | trigger | | Removes config from MQTT |

**On Publish:** Serializes to JSON and publishes to `DMX/Universe/{name}`:

```json
{
    "description": "...",
    "controller": "10.0.1.228",
    "net": 0,
    "subnet": 0,
    "universe": 0,
    "channels": 306,
    "log": false,
    "disable_send": false
}
```

Sets Status = "Published".

**On Unpublish:** Publishes empty message to `DMX/Universe/{name}`. Sets Status = "Unpublished".

**On any property change:** Sets Status = "Modified" (if currently "Published").

### Array

The primary unit of control. Each array is a BindTarget that equipment (DimmedLight, etc.) can bind to.

**Traits:**

| Trait | Purpose |
|-------|---------|
| `BindTarget` | Equipment can bind to this |
| `has PowerState` | Power on/off |
| `has Dimmer` | Intensity 0.0-1.0 |
| `has Fade` | Fade time for transitions |

**Properties:**

| Property | Type | Default | Notes |
|----------|------|---------|-------|
| `UniverseId` | `String` | (required) | Default universe for channel references |
| `Description` | `String` | (required) | Human-readable name |
| `OnEffect` | `String?` | `"on"` | Effect ID for On command |
| `OffEffect` | `String?` | `"off"` | Effect ID for Off command |
| `DimEffect` | `String?` | `"dim"` | Effect ID for Dim command |
| `Status` | `String? managed memoryonly` | | Modified / Published / Unpublished |
| `Publish` | trigger | | Publishes config to MQTT |
| `Unpublish` | trigger | | Removes config from MQTT |

**Contains:** LightGroup, ArrayEffect, ArrayValue children.

**On Publish:** Assembles the full array definition JSON from children:
- `lights` map from LightGroup children (name → Channels string)
- `effects` map from ArrayEffect children (name → serialized effect tree)
- `default_values` map from ArrayValue children (name → Value string)

Publishes to `DMX/Array/{name}`. Sets Status = "Published".

**On Unpublish:** Publishes empty to `DMX/Array/{name}`. Sets Status = "Unpublished".

**On child or property change:** Sets Status = "Modified" (if currently "Published").

**Command behavior (always active, regardless of Status):**

| Property Change | MQTT Action |
|----------------|-------------|
| `Power = true` | Publish to `DMX/Command/On`: `{"array_id": "{name}", "dimming_amount": Intensity * 1000}` |
| `Power = false` | Publish to `DMX/Command/Off`: `{"array_id": "{name}"}` |
| `Intensity changed` | Publish to `DMX/Command/Dim`: `{"array_id": "{name}", "dimming_amount": Intensity * 1000}`. Also sets Power = true if Intensity > 0. |

### LightGroup

A named set of DMX channels within an array.

**Properties:**

| Property | Type | Notes |
|----------|------|-------|
| `Channels` | `String` | Channel definition in mqtt_dmx syntax |

**Channel syntax reference:**
- `s:N` — single channel at address N
- `rgb:N` — RGB at 3 consecutive channels starting at N
- `rgb:R/G/B` — RGB at specific addresses
- `w:N` — TriWhite at 3 consecutive channels starting at N
- `w:W1/W2/W3` — TriWhite at specific addresses
- `@name` — reference another light group
- `$id` — switch subsequent channels to universe id

The group named `all` must encompass every channel. This is enforced by mqtt_dmx at publish time.

### Effect Node Classes

Abstract base and four concrete effect types, all containable for building nested trees.

#### EffectNode (abstract)

Common base class. No properties.

#### FadeEffect

| Property | Type | Notes |
|----------|------|-------|
| `Lights` | `String` | Light group expression (e.g. `"@all"`, `"@center"`) |
| `Ticks` | `String` | Number of ticks or variable (e.g. `"20"`, `` "`on_ticks=10`" ``) |
| `Target` | `String` | Target values or variable (e.g. `"s(255);rgb(255,255,255)"`) |
| `NoDimming` | `Boolean?` | If true, ignore dimming amount |

Serializes to: `{"type": "fade", "lights": "...", "ticks": ..., "target": "...", "no_dimming": ...}`

Note: `Ticks` is String to support backtick variable expressions. When serializing, attempt to parse as integer first; if it's numeric, emit as a number in JSON.

#### DelayEffect

| Property | Type | Notes |
|----------|------|-------|
| `Ticks` | `String` | Number of ticks or variable expression |

Serializes to: `{"type": "delay", "ticks": ...}`

#### SequenceEffect

Contains child EffectNode objects. Executes them one after another.

Serializes to: `{"type": "sequence", "nodes": [...]}`

#### ParallelEffect

Contains child EffectNode objects. Executes them simultaneously.

Serializes to: `{"type": "parallel", "nodes": [...]}`

### ArrayEffect

A named effect local to an array. Contains exactly one root EffectNode child (which may be a Sequence/Parallel containing further children).

The name of the ArrayEffect becomes the key in the array's `effects` map.

### GlobalEffect

A named global effect, defined under the top-level Effects container.

**Properties:**

| Property | Type | Notes |
|----------|------|-------|
| `Status` | `String? managed memoryonly` | Modified / Published / Unpublished |
| `Publish` | trigger | Publishes to MQTT |
| `Unpublish` | trigger | Removes from MQTT |

Contains one root EffectNode child. Published to `DMX/Effect/{name}`.

### GlobalValue

A named global variable value.

**Properties:**

| Property | Type | Notes |
|----------|------|-------|
| `Value` | `String` | The value content |
| `Status` | `String? managed memoryonly` | Modified / Published / Unpublished |
| `Publish` | trigger | Publishes to MQTT |
| `Unpublish` | trigger | Removes from MQTT |

Published to `DMX/Value/{name}` as: `{"value": "..."}`.

### ArrayValue

A default variable value local to an array.

**Properties:**

| Property | Type | Notes |
|----------|------|-------|
| `Value` | `String` | The default value |

The name becomes the key in the array's `default_values` map.

## MQTT Topic Map

### Published by SDL module

| Topic | Trigger | Payload |
|-------|---------|---------|
| `DMX/Universe/{name}` | Universe.Publish | Universe definition JSON |
| `DMX/Universe/{name}` | Universe.Unpublish | Empty (removes) |
| `DMX/Array/{name}` | Array.Publish | Array definition JSON |
| `DMX/Array/{name}` | Array.Unpublish | Empty (removes) |
| `DMX/Effect/{name}` | GlobalEffect.Publish | Effect definition JSON |
| `DMX/Effect/{name}` | GlobalEffect.Unpublish | Empty (removes) |
| `DMX/Value/{name}` | GlobalValue.Publish | Value definition JSON |
| `DMX/Value/{name}` | GlobalValue.Unpublish | Empty (removes) |
| `DMX/Command/On` | Array.Power = true | `{"array_id": "...", "dimming_amount": N}` |
| `DMX/Command/Off` | Array.Power = false | `{"array_id": "..."}` |
| `DMX/Command/Dim` | Array.Intensity changed | `{"array_id": "...", "dimming_amount": N}` |

### Subscribed by SDL module

| Topic | Handler | Updates |
|-------|---------|---------|
| `DMX/Active` | OnActive | `DMX.Active` |
| `DMX/LastError` | OnLastError | `DMX.LastError`, `DMX.LastErrorTime` |
