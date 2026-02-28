# DMX SDL Module Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement `dmx.sdl`, a Store server SDL module that manages DMX lighting configuration and control via the mqtt_dmx MQTT bridge.

**Architecture:** A single SDL file defining a class hierarchy: DMX root → container classes → Universe, Array, GlobalEffect, GlobalValue. Arrays are BindTargets with PowerState/Dimmer/Fade. All config objects use explicit Publish/Unpublish triggers. Publish status is indicated by a suffix on the Description property (`" - (modified)"`, `" - (unpublished)"`), stripped on publish. Effects are modeled as nested SDL object trees (Fade, Delay, Sequence, Parallel).

**Tech Stack:** SDL (Store Definition Language), Rhai scripting, MQTT integration via `mqtt::publish()` and `mqtt::topic_handlers()`

**Reference files:**
- Design: `docs/plans/2026-02-28-dmx-sdl-design.md`
- Existing SDL modules: `/Users/yuval/Documents/Projects/Store/store_server/modules/dali.sdl`, `hdl.sdl`, `lutron/module.sdl`
- Component mixins: `/Users/yuval/Documents/Projects/Store/store_server/schemas/HA/component.sdl`
- MQTT infrastructure: `/Users/yuval/Documents/Projects/Store/store_server/schemas/Std/mqtt.sdl`
- mqtt_dmx protocol: `docs/mqtt-dmx-documentation.md`

**Output file:** `/Users/yuval/Documents/Projects/Store/store_server/modules/dmx.sdl`

---

### Task 1: Icons and DMX Root Class

**Files:**
- Create: `/Users/yuval/Documents/Projects/Store/store_server/modules/dmx.sdl`

**Step 1: Create the SDL file with icons and root class**

```sdl
icons {
    // DMX — stage light beam
    icon_dmx = ```svg
    <svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
      <rect x="7" y="2" width="10" height="6" rx="1" stroke="currentColor" stroke-width="1.5"/>
      <path d="M9 8l-3 14h12l-3-14" stroke="currentColor" stroke-width="1.5" fill="none"/>
      <line x1="12" y1="8" x2="12" y2="5" stroke="currentColor" stroke-width="1.5"/>
    </svg>
    ```

    // Universe — network globe
    icon_universe = ```svg
    <svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
      <circle cx="12" cy="12" r="9" stroke="currentColor" stroke-width="1.5"/>
      <ellipse cx="12" cy="12" rx="4" ry="9" stroke="currentColor" stroke-width="1.5"/>
      <line x1="3" y1="12" x2="21" y2="12" stroke="currentColor" stroke-width="1.5"/>
    </svg>
    ```

    // Array — grid of lights
    icon_array = ```svg
    <svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
      <rect x="3" y="3" width="7" height="7" rx="1" stroke="currentColor" stroke-width="1.5"/>
      <rect x="14" y="3" width="7" height="7" rx="1" stroke="currentColor" stroke-width="1.5"/>
      <rect x="3" y="14" width="7" height="7" rx="1" stroke="currentColor" stroke-width="1.5"/>
      <rect x="14" y="14" width="7" height="7" rx="1" stroke="currentColor" stroke-width="1.5"/>
    </svg>
    ```

    // Effect — waveform
    icon_effect = ```svg
    <svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
      <path d="M3 12c2-4 4-8 6 0s4 4 6 0 4-8 6 0" stroke="currentColor" stroke-width="1.5" fill="none"/>
    </svg>
    ```

    // Value — variable tag
    icon_value = ```svg
    <svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
      <path d="M9 4l-5 8 5 8" stroke="currentColor" stroke-width="1.5" fill="none"/>
      <path d="M15 4l5 8-5 8" stroke="currentColor" stroke-width="1.5" fill="none"/>
      <line x1="14" y1="6" x2="10" y2="18" stroke="currentColor" stroke-width="1.5"/>
    </svg>
    ```

    // Container — folder
    icon_container = ```svg
    <svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
      <path d="M3 7V5a2 2 0 012-2h4l2 2h8a2 2 0 012 2v12a2 2 0 01-2 2H5a2 2 0 01-2-2V7z" stroke="currentColor" stroke-width="1.5" fill="none"/>
    </svg>
    ```

    // Light group — layers
    icon_light_group = ```svg
    <svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
      <path d="M12 4l8 4-8 4-8-4 8-4z" stroke="currentColor" stroke-width="1.5" fill="none"/>
      <path d="M4 12l8 4 8-4" stroke="currentColor" stroke-width="1.5" fill="none"/>
      <path d="M4 16l8 4 8-4" stroke="currentColor" stroke-width="1.5" fill="none"/>
    </svg>
    ```
}

module {
    description: "DMX lighting control via mqtt_dmx Art-Net bridge"

    // Strip " - (modified)" or " - (unpublished)" suffix from a description string.
    fn strip_status_suffix(desc) {
        if desc == () { return ""; }
        let s = desc.to_string();
        if s.ends_with(" - (modified)") {
            s.sub_string(0, s.len() - 13)
        } else if s.ends_with(" - (unpublished)") {
            s.sub_string(0, s.len() - 16)
        } else {
            s
        }
    }

    // Return true if the description already has a status suffix.
    fn has_status_suffix(desc) {
        if desc == () { return false; }
        let s = desc.to_string();
        s.ends_with(" - (modified)") || s.ends_with(" - (unpublished)")
    }

    // Root device object — one per system.
    // Subscribes to mqtt_dmx status topics for service health monitoring.
    class DMX is DeviceObject {
        is #Mqtt/UsingMqtt
        is #Mqtt/MqttSubscriber
        icon: icon_dmx
        description: "DMX lighting controller via mqtt_dmx Art-Net bridge"

        exposed contains DmxUniverses
        exposed contains DmxArrays
        exposed contains DmxEffects
        exposed contains DmxValues

        Active: Boolean? memoryonly managed description: "mqtt_dmx service is running"
        LastError: String? memoryonly managed description: "Last error from mqtt_dmx"
        LastErrorTime: String? memoryonly managed description: "Timestamp of last error"

        on Created {
            if self.children.is_empty() {
                self.create(#{ "Name": "Universes", "Class": "*Dmx/DmxUniverses" });
                self.create(#{ "Name": "Arrays", "Class": "*Dmx/DmxArrays" });
                self.create(#{ "Name": "Effects", "Class": "*Dmx/DmxEffects" });
                self.create(#{ "Name": "Values", "Class": "*Dmx/DmxValues" });
            }
        }

        fn TopicHandlers() {
            mqtt::topic_handlers(self, #{
                "DMX/Active": "OnActive",
                "DMX/LastError": "OnLastError"
            })
        }

        fn OnActive(topic: String, message: Blob) {
            let value = message.as_string();
            self["Active"] = value == "true";
        }

        fn OnLastError(topic: String, message: Blob) {
            let error = parse_json(message.as_string());
            self["LastError"] = error.message;
            self["LastErrorTime"] = error.time;
        }
    }
}
```

**Step 2: Review the root class**

Verify:
- `is DeviceObject` makes it a top-level infrastructure object
- `is #Mqtt/UsingMqtt` provides MqttClient reference property
- `is #Mqtt/MqttSubscriber` enables TopicHandlers
- `on Created` auto-creates the four containers
- `OnActive` and `OnLastError` parse incoming MQTT messages

**Step 3: Commit**

```bash
git add /Users/yuval/Documents/Projects/Store/store_server/modules/dmx.sdl
git commit -m "feat(dmx): add icons and DMX root class with MQTT status subscription"
```

---

### Task 2: Container Classes and Universe

**Files:**
- Modify: `/Users/yuval/Documents/Projects/Store/store_server/modules/dmx.sdl`

**Step 1: Add container classes and Universe inside the `module { }` block**

Add after the DMX class closing brace, still inside `module { }`:

```sdl
    // Container for universe definitions.
    class DmxUniverses {
        icon: icon_container
        description: "Art-Net universe definitions"
        exposed contains Universe
    }

    // Container for array definitions.
    class DmxArrays {
        icon: icon_container
        description: "DMX array definitions"
        exposed contains DmxArray
    }

    // Container for global effect definitions.
    class DmxEffects {
        icon: icon_container
        description: "Global effect definitions"
        exposed contains GlobalEffect
    }

    // Container for global value definitions.
    class DmxValues {
        icon: icon_container
        description: "Global value definitions"
        exposed contains GlobalValue
    }

    // A single Art-Net DMX universe with up to 512 channels.
    class Universe {
        icon: icon_universe
        description: "Art-Net DMX universe"

        Description: String? description: "Human-readable name"
        ControllerAddress: String description: "Art-Net node IP address"
        Net: Int { min: 0, max: 127, description: "Art-Net net number" }
        Subnet: Int { min: 0, max: 15, description: "Art-Net subnet number" }
        UniverseNumber: Int { min: 0, max: 15, description: "Art-Net universe number" }
        Channels: Int { min: 1, max: 512, description: "Number of DMX channels" }
        Log: Boolean? description: "Enable channel-level logging"
        DisableSend: Boolean? description: "Skip sending packets (testing)"

        Publish: Boolean? trigger description: "Publish universe config to mqtt_dmx"
        Unpublish: Boolean? trigger description: "Remove universe from mqtt_dmx"

        fn MarkModified() {
            if !has_status_suffix(self["Description"]) {
                self["Description"] = self["Description"].to_string() + " - (modified)";
            }
        }

        on PropertyChanged(ControllerAddress) { self.call_method("MarkModified", #{}); }
        on PropertyChanged(Net) { self.call_method("MarkModified", #{}); }
        on PropertyChanged(Subnet) { self.call_method("MarkModified", #{}); }
        on PropertyChanged(UniverseNumber) { self.call_method("MarkModified", #{}); }
        on PropertyChanged(Channels) { self.call_method("MarkModified", #{}); }
        on PropertyChanged(Log) { self.call_method("MarkModified", #{}); }
        on PropertyChanged(DisableSend) { self.call_method("MarkModified", #{}); }

        on PropertyChanged(Publish) {
            let clean_desc = strip_status_suffix(self["Description"]);
            self["Description"] = clean_desc;

            let definition = #{
                "description": clean_desc,
                "controller": self["ControllerAddress"],
                "net": self["Net"],
                "subnet": self["Subnet"],
                "universe": self["UniverseNumber"],
                "channels": self["Channels"],
            };

            if self["Log"] {
                definition["log"] = true;
            }
            if self["DisableSend"] {
                definition["disable_send"] = true;
            }

            mqtt::publish(self, `DMX/Universe/${self.name}`, to_json_blob(definition), true);
        }

        on PropertyChanged(Unpublish) {
            mqtt::publish(self, `DMX/Universe/${self.name}`, blob(), true);
            let clean_desc = strip_status_suffix(self["Description"]);
            self["Description"] = clean_desc + " - (unpublished)";
        }
    }
```

**Step 2: Review**

Verify:
- Container classes only have `exposed contains` and metadata
- Universe properties match mqtt_dmx `UniverseDefinition` JSON schema
- `MarkModified()` appends `" - (modified)"` only if no suffix already present
- Publish handler strips suffix from Description, uses clean description in JSON, publishes with `retain: true`
- Unpublish sends an empty blob with `retain: true` and appends `" - (unpublished)"` to Description
- Optional fields (`log`, `disable_send`) only included when true
- Description PropertyChanged is NOT hooked to MarkModified (user edits to description shouldn't auto-mark)

**Step 3: Commit**

```bash
git commit -am "feat(dmx): add container classes and Universe with publish/unpublish"
```

---

### Task 3: Effect Node Classes

**Files:**
- Modify: `/Users/yuval/Documents/Projects/Store/store_server/modules/dmx.sdl`

**Step 1: Add effect node classes inside `module { }`**

Add after the Universe class:

```sdl
    // Abstract base for all effect nodes in the effect tree.
    abstract class EffectNode { }

    // Smoothly interpolates channel values from current to target over ticks.
    class FadeEffect is EffectNode {
        icon: icon_effect
        description: "Fade light values to a target over time"

        Lights: String description: "Light group expression (e.g. @all, @center)"
        Ticks: String description: "Duration in ticks or variable (e.g. 20, `on_ticks=10`)"
        Target: String description: "Target values or variable (e.g. s(255);rgb(255,255,255))"
        NoDimming: Boolean? description: "If true, ignore dimming amount"
    }

    // Pauses execution for a number of ticks. Used in sequences.
    class DelayEffect is EffectNode {
        icon: icon_effect
        description: "Pause for a number of ticks"

        Ticks: String description: "Duration in ticks or variable expression"
    }

    // Executes child effects one after another.
    class SequenceEffect is EffectNode {
        icon: icon_effect
        description: "Execute child effects in sequence"

        contains FadeEffect
        contains DelayEffect
        contains SequenceEffect
        contains ParallelEffect
    }

    // Executes all child effects simultaneously.
    class ParallelEffect is EffectNode {
        icon: icon_effect
        description: "Execute child effects in parallel"

        contains FadeEffect
        contains DelayEffect
        contains SequenceEffect
        contains ParallelEffect
    }
```

**Step 2: Add the effect serialization helper function**

Add inside `module { }` (module-level function, not inside a class):

```sdl
    // Recursively serialize an effect node to a JSON-compatible map.
    fn serialize_effect(node) {
        if node.is_of_type("*Dmx/FadeEffect") {
            let effect = #{
                "type": "fade",
                "lights": node["Lights"],
                "ticks": node["Ticks"],
                "target": node["Target"],
            };

            // Try to parse ticks as integer
            let ticks_int = node["Ticks"].parse_int();
            if ticks_int != () {
                effect["ticks"] = ticks_int;
            }

            if node["NoDimming"] {
                effect["no_dimming"] = true;
            }

            effect
        } else if node.is_of_type("*Dmx/DelayEffect") {
            let effect = #{
                "type": "delay",
                "ticks": node["Ticks"],
            };

            let ticks_int = node["Ticks"].parse_int();
            if ticks_int != () {
                effect["ticks"] = ticks_int;
            }

            effect
        } else if node.is_of_type("*Dmx/SequenceEffect") {
            let nodes = [];
            for child in node.children {
                nodes.push(serialize_effect(child));
            }
            #{ "type": "sequence", "nodes": nodes }
        } else if node.is_of_type("*Dmx/ParallelEffect") {
            let nodes = [];
            for child in node.children {
                nodes.push(serialize_effect(child));
            }
            #{ "type": "parallel", "nodes": nodes }
        }
    }
```

**Step 3: Review**

Verify:
- EffectNode is abstract (no instances, just a type marker)
- SequenceEffect and ParallelEffect contain all four effect types (including themselves for nesting)
- `serialize_effect()` recursively builds JSON maps
- Ticks parsed as int when possible, kept as string (for variable syntax) otherwise
- NoDimming only included when true

**Step 4: Commit**

```bash
git commit -am "feat(dmx): add effect node classes and serialization helper"
```

---

### Task 4: LightGroup, ArrayValue, and ArrayEffect

**Files:**
- Modify: `/Users/yuval/Documents/Projects/Store/store_server/modules/dmx.sdl`

**Step 1: Add LightGroup, ArrayValue, and ArrayEffect classes inside `module { }`**

```sdl
    // A named set of DMX channels within an array.
    // Uses mqtt_dmx native syntax: s:N, rgb:N, w:N, @group, $universe
    class LightGroup {
        icon: icon_light_group
        description: "Named group of DMX channels"

        Channels: String description: "Channel definitions (e.g. rgb:1,rgb:4,s:7,@center,$2,w:100)"
    }

    // A default variable value local to an array.
    // The object name becomes the variable name in the default_values map.
    class ArrayValue {
        icon: icon_value
        description: "Default variable value for this array"

        Value: String description: "The default value"
    }

    // A named effect local to an array.
    // Contains one root effect node (which may be a Sequence/Parallel with children).
    // The object name becomes the key in the array's effects map.
    class ArrayEffect {
        icon: icon_effect
        description: "Named effect definition for this array"

        contains FadeEffect
        contains DelayEffect
        contains SequenceEffect
        contains ParallelEffect
    }
```

**Step 2: Review**

Verify:
- LightGroup stores channels as a single string in mqtt_dmx syntax
- ArrayValue stores a single value string; the object name is the variable name
- ArrayEffect can contain any effect node type as its root

**Step 3: Commit**

```bash
git commit -am "feat(dmx): add LightGroup, ArrayValue, and ArrayEffect classes"
```

---

### Task 5: Array Class with Commands and Publishing

**Files:**
- Modify: `/Users/yuval/Documents/Projects/Store/store_server/modules/dmx.sdl`

**Step 1: Add the Array class inside `module { }`**

```sdl
    // Primary unit of control. A named group of lights with effects and default values.
    // Equipment (DimmedLight, etc.) binds to this via BindTarget.
    class DmxArray is BindTarget {
        has #Component/PowerState
        has #Component/Dimmer
        has #Component/Fade
        icon: icon_array
        description: "DMX light array with effects and channel groups"

        Description: String? description: "Human-readable name"
        UniverseId: String description: "Default universe ID for channel references"
        OnEffect: String? description: "Effect ID for On command (default: on)"
        OffEffect: String? description: "Effect ID for Off command (default: off)"
        DimEffect: String? description: "Effect ID for Dim command (default: dim)"

        Publish: Boolean? trigger description: "Publish array config to mqtt_dmx"
        Unpublish: Boolean? trigger description: "Remove array from mqtt_dmx"

        exposed contains LightGroup
        exposed contains ArrayEffect
        exposed contains ArrayValue

        fn MarkModified() {
            if !has_status_suffix(self["Description"]) {
                self["Description"] = self["Description"].to_string() + " - (modified)";
            }
        }

        on PropertyChanged(UniverseId) { self.call_method("MarkModified", #{}); }
        on PropertyChanged(OnEffect) { self.call_method("MarkModified", #{}); }
        on PropertyChanged(OffEffect) { self.call_method("MarkModified", #{}); }
        on PropertyChanged(DimEffect) { self.call_method("MarkModified", #{}); }

        on PropertyChanged(Publish) {
            let clean_desc = strip_status_suffix(self["Description"]);
            self["Description"] = clean_desc;

            // Build lights map from LightGroup children
            let lights = #{};
            for group in self.children_of_type("*Dmx/LightGroup") {
                lights[group.name] = group["Channels"];
            }

            // Build effects map from ArrayEffect children
            let effects = #{};
            for effect in self.children_of_type("*Dmx/ArrayEffect") {
                // Serialize the first child effect node
                let children = effect.children;
                if children.len() > 0 {
                    effects[effect.name] = serialize_effect(children[0]);
                }
            }

            // Build default_values map from ArrayValue children
            let default_values = #{};
            for val in self.children_of_type("*Dmx/ArrayValue") {
                default_values[val.name] = val["Value"];
            }

            let definition = #{
                "universe_id": self["UniverseId"],
                "description": clean_desc,
                "lights": lights,
            };

            if self["OnEffect"] != () && self["OnEffect"] != "" {
                definition["on"] = self["OnEffect"];
            }
            if self["OffEffect"] != () && self["OffEffect"] != "" {
                definition["off"] = self["OffEffect"];
            }
            if self["DimEffect"] != () && self["DimEffect"] != "" {
                definition["dim"] = self["DimEffect"];
            }

            if effects.len() > 0 {
                definition["effects"] = effects;
            }
            if default_values.len() > 0 {
                definition["default_values"] = default_values;
            }

            mqtt::publish(self, `DMX/Array/${self.name}`, to_json_blob(definition), true);
        }

        on PropertyChanged(Unpublish) {
            mqtt::publish(self, `DMX/Array/${self.name}`, blob(), true);
            let clean_desc = strip_status_suffix(self["Description"]);
            self["Description"] = clean_desc + " - (unpublished)";
        }

        // Command: turn on
        on PropertyChanged(Power) {
            let dmx = self.get_ancestor_of_type("*Dmx/DMX");
            if new_value {
                let command = #{
                    "array_id": self.name,
                };
                let intensity = self["Intensity"];
                if intensity != () {
                    command["dimming_amount"] = (intensity * 1000).round().to_int();
                }
                mqtt::publish(dmx, "DMX/Command/On", to_json_blob(command));
            } else {
                let command = #{
                    "array_id": self.name,
                };
                mqtt::publish(dmx, "DMX/Command/Off", to_json_blob(command));
            }
        }

        // Command: dim
        on PropertyChanged(Intensity) {
            if new_value > 0.0 && !self["Power"] {
                self["Power"] = true;
            } else if new_value > 0.0 {
                let dmx = self.get_ancestor_of_type("*Dmx/DMX");
                let command = #{
                    "array_id": self.name,
                    "dimming_amount": (new_value * 1000).round().to_int(),
                };
                mqtt::publish(dmx, "DMX/Command/Dim", to_json_blob(command));
            }
        }
    }
```

**Step 2: Review**

Verify:
- `is BindTarget` so equipment can bind to it
- `has #Component/PowerState`, `Dimmer`, `Fade` provide the standard light control interface
- Has `Description` property for status suffix tracking
- Publish handler strips suffix, uses clean description in JSON, assembles from children using `children_of_type()`
- Unpublish appends `" - (unpublished)"` to Description
- `serialize_effect()` call for each ArrayEffect child
- Power on → publishes On command with dimming_amount from current Intensity
- Power off → publishes Off command
- Intensity change when already on → publishes Dim command
- Intensity change when off → sets Power = true (which triggers the On command)
- Commands use `self.get_ancestor_of_type("*Dmx/DMX")` to find the root for MQTT publishing

**Step 3: Commit**

```bash
git commit -am "feat(dmx): add Array class with publish/unpublish and On/Off/Dim commands"
```

---

### Task 6: GlobalEffect and GlobalValue

**Files:**
- Modify: `/Users/yuval/Documents/Projects/Store/store_server/modules/dmx.sdl`

**Step 1: Add GlobalEffect and GlobalValue classes inside `module { }`**

```sdl
    // A named global effect definition.
    // Published to DMX/Effect/{name} for use by any array.
    class GlobalEffect {
        icon: icon_effect
        description: "Global effect definition available to all arrays"

        Description: String? description: "Human-readable name"
        Publish: Boolean? trigger description: "Publish effect to mqtt_dmx"
        Unpublish: Boolean? trigger description: "Remove effect from mqtt_dmx"

        contains FadeEffect
        contains DelayEffect
        contains SequenceEffect
        contains ParallelEffect

        on PropertyChanged(Publish) {
            let clean_desc = strip_status_suffix(self["Description"]);
            self["Description"] = clean_desc;

            let children = self.children;
            if children.len() > 0 {
                let effect = serialize_effect(children[0]);
                mqtt::publish(self, `DMX/Effect/${self.name}`, to_json_blob(effect), true);
            }
        }

        on PropertyChanged(Unpublish) {
            mqtt::publish(self, `DMX/Effect/${self.name}`, blob(), true);
            let clean_desc = strip_status_suffix(self["Description"]);
            self["Description"] = clean_desc + " - (unpublished)";
        }
    }

    // A named global variable value.
    // Published to DMX/Value/{name} for use in effect variable expressions.
    class GlobalValue {
        icon: icon_value
        description: "Global variable value for effect parameterization"

        Description: String? description: "Human-readable name"
        Value: String description: "The value content"

        Publish: Boolean? trigger description: "Publish value to mqtt_dmx"
        Unpublish: Boolean? trigger description: "Remove value from mqtt_dmx"

        on PropertyChanged(Value) {
            if !has_status_suffix(self["Description"]) {
                self["Description"] = self["Description"].to_string() + " - (modified)";
            }
        }

        on PropertyChanged(Publish) {
            let clean_desc = strip_status_suffix(self["Description"]);
            self["Description"] = clean_desc;

            let definition = #{
                "value": self["Value"],
            };
            mqtt::publish(self, `DMX/Value/${self.name}`, to_json_blob(definition), true);
        }

        on PropertyChanged(Unpublish) {
            mqtt::publish(self, `DMX/Value/${self.name}`, blob(), true);
            let clean_desc = strip_status_suffix(self["Description"]);
            self["Description"] = clean_desc + " - (unpublished)";
        }
    }
```

**Step 2: Review**

Verify:
- GlobalEffect publishes the serialized first child effect node
- GlobalValue publishes `{"value": "..."}` matching mqtt_dmx's `ValueDefinition`
- Both have Description property with suffix-based status tracking
- Publish strips suffix from Description
- Unpublish appends `" - (unpublished)"` to Description
- GlobalValue marks Description as modified when Value changes

**Step 3: Commit**

```bash
git commit -am "feat(dmx): add GlobalEffect and GlobalValue with publish/unpublish"
```

---

### Task 7: Final Review and Integration Test

**Files:**
- Review: `/Users/yuval/Documents/Projects/Store/store_server/modules/dmx.sdl`

**Step 1: Review the complete file**

Read through the entire file and verify:
- All classes are inside the `module { }` block
- `icons { }` block is before `module { }`
- No syntax errors (matching braces, correct SDL syntax)
- All `contains` declarations match actual class names
- All cross-references use `*Dmx/ClassName` format
- `serialize_effect()` function is at module level, not inside a class
- MQTT topics match mqtt_dmx documentation exactly

**Step 2: Verify class hierarchy completeness**

Check that every class from the design doc is implemented:
- [ ] DMX (root)
- [ ] DmxUniverses, DmxArrays, DmxEffects, DmxValues (containers)
- [ ] Universe
- [ ] DmxArray
- [ ] LightGroup
- [ ] ArrayEffect
- [ ] ArrayValue
- [ ] EffectNode (abstract)
- [ ] FadeEffect, DelayEffect, SequenceEffect, ParallelEffect
- [ ] GlobalEffect
- [ ] GlobalValue

**Step 3: Verify MQTT topic coverage**

Check against design doc MQTT topic map:
- [ ] `DMX/Universe/{name}` — published by Universe.Publish/Unpublish
- [ ] `DMX/Array/{name}` — published by DmxArray.Publish/Unpublish
- [ ] `DMX/Effect/{name}` — published by GlobalEffect.Publish/Unpublish
- [ ] `DMX/Value/{name}` — published by GlobalValue.Publish/Unpublish
- [ ] `DMX/Command/On` — published by DmxArray.Power=true
- [ ] `DMX/Command/Off` — published by DmxArray.Power=false
- [ ] `DMX/Command/Dim` — published by DmxArray.Intensity changed
- [ ] `DMX/Active` — subscribed by DMX root
- [ ] `DMX/LastError` — subscribed by DMX root

**Step 4: Commit final version**

```bash
git commit -am "feat(dmx): complete dmx.sdl module - final review pass"
```
