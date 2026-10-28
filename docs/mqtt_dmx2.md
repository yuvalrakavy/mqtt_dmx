# mqtt_dmx2 bridge

`mqtt_dmx2` is the BridgeKit DMX bridge. The legacy `mqtt_dmx` binary and `legacy-v1`
tag remain available. Use separate MQTT brokers and service/fallback paths for v1
and v2; their MQTT contracts are incompatible. There is no v1 file migration.

```sh
mqtt_dmx2 --instance living-room --broker broker-v2:1883 \
  --root DMX --startup-fallback /var/lib/mqtt_dmx2/living-room.json
```

`--instance` selects the room/installation. The root defaults to `DMX`; `--root`
overrides it. Without `--startup-fallback`, the explicit-save path is
`mqtt_dmx2-<instance>-startup.json` in the working directory. Startup only reads
that path. Normal requests, completions and shutdown never write it.

The managed lighting service runs independently of MQTT. Every connection reads
the retained `DMX/State/<instance>/_recovery` carrier before publishing recovery
or accepting ordinary control. A running saved animation starts again from its
original initial output; completed/stopped snapshots restore final/frozen output.
Ordinary MQTT reconnect preserves live playback. A valid local startup fallback
can begin after five seconds without broker recovery; matching completed fallback
output is preserved and promoted when MQTT returns.

Store does not keep a second recovery snapshot. With no usable broker/local state,
the empty bridge sends no unknown DMX addresses. Read-only `ReadRecovery` returns
transient process, generation and mode metadata. It needs no prior generation and
accepts no target or parameters. `Initialize` takes a complete configuration and
the discovered `target-generation` MQTT v5 user property, starts configured arrays
Off, and returns a fresh accepted configuration/history/authority/generation table.
Initialization is allowed only on an empty or faulted engine. Retained intent from
before initialization cannot bypass the new identities.

Publish definitions on `Config/{instance}/configuration` only when ready to apply
them. Draft edits stay in Store. The bridge assigns each array's mapping generation
from the resolved physical channels: existing mappings retain their generation and
running execution; changed mappings receive fresh generations and require completed
Off. New arrays start Off. Configuration request generations are compatibility
placeholders, never authority chosen by the publisher.

The bridge refuses a configuration with unresolved universe, light-group or
selected-effect references, preserving the accepted configuration and running
lighting. Renaming a universe while an array still refers to its old name is
therefore refused; publication does not automatically rewrite references.

The accepted configuration in `State/{instance}/configuration` contains the assigned
generations and its recalculated content digest. Publication comparison ignores only
those generations and their derived digest; authored content and ordering still match
exactly. Retrying the original publication, including after recovery, keeps the
accepted mapping and execution IDs. Store uses the returned generations for playback.
The configuration and playback formats are defined by `dmx_protocol`. Configuration
uses `Config/<instance>/configuration`. Playback is one complete canonical JSON
**string** on `Desired/<instance>/<array UUID>/playback`, with finite MQTT expiry and
the array's `target-generation` user property. Level zero means Off; positive levels
mean On. Live brightness changes preserve the animation timeline.

Commands use `Command/<instance>`, are never retained and use response-topic and
correlation-data for replies. `Stop`/`Restart` carry the ordered operation and
expected execution/playback identities. `SaveStartupFallback` carries `save_id`
(a UUID) and uses the current instance generation. It writes only the captured
immutable snapshot and acknowledges file/directory synchronization; failure does
not change lighting. A save is not implicit on shutdown.

Reported output is assumed actuator state, not confirmation from physical fixtures.
Broker acceptance is not broker disk durability. Encoded documents, request queues,
configuration shape and individual players retain their admission limits. Exact
combined memory accounting across all service/recovery copies is deferred; there
is no total-process 16 MiB/RSS guarantee.

The Store driver conversion and complete cross-process acceptance remain separate
work. The bridge's integration tests use owned ephemeral MQTT and UDP loopback
receivers; they do not require or contact lighting hardware.
