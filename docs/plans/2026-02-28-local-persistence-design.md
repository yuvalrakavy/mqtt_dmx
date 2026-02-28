# Local Configuration Persistence

## Goal

Protect against configuration loss when MQTT retained messages are missing (publisher didn't set retained flag, broker restarted without persistence). Local files act as a fallback — MQTT always takes priority when messages arrive.

## Storage

```
<storage_path>/          # default: ./dmx_config
  universes.json         # { "universe_id": UniverseDefinition, ... }
  arrays.json            # { "array_id": DmxArray, ... }
  effects.json           # { "effect_id": EffectNodeDefinition, ... }
  values.json            # { "value_name": "value_string", ... }
```

## Configuration

Storage path resolution order:
1. CLI argument: `--storage <path>`
2. Environment variable: `MQTT_DMX_STORAGE_PATH`
3. Default: `./dmx_config`

## Architecture

### New module: `persistence`

Provides `load_*` and `save_*` functions for each entity type. Pure functions operating on file I/O, no async needed (files are small).

### Startup flow

1. `Service::start()` creates managers and channels, spawns workers
2. Loads persisted state via the `persistence` module
3. Sends `AddUniverse`, `AddArray`, `AddEffect`, `AddGlobalValue` messages through existing channels
4. Spawns MQTT worker — incoming retained messages overwrite persisted entries

### Runtime write flow

1. MQTT subscriber receives a config message (add/remove Universe/Array/Effect/Value)
2. Sends to the appropriate manager, receives OK reply (existing flow)
3. Calls `persistence::save_*` to write the updated state to disk (new step)

### Write timing

Writes happen immediately on each config change. Configuration changes are infrequent (setting up lighting topology, not per-frame operations), so I/O volume is negligible. Immediate writes protect against unexpected crashes.

### Error handling

Persistence errors are logged and reported via the MQTT error topic but do not fail the operation. The in-memory state is the primary source of truth during runtime.

## Required code changes

- Add `Serialize` derive to `DmxArray` and `EffectNodeDefinition` (and their nested types)
- New `persistence` module with load/save functions
- Add `--storage` CLI argument and `MQTT_DMX_STORAGE_PATH` env var support
- Add `storage_path` to `ServiceConfig`
- Pass persistence handle to MQTT subscriber
- Add persistence save calls after successful add/remove operations in MQTT subscriber
- Add persistence load + replay in `Service::start()`
