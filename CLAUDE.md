# mqtt_dmx

Two crates: `mqtt_dmx/` (the bridge — cross-compiled for Raspberry Pi armv7 by default via `.cargo/config.toml`) and `artnet_emulator/` (a dev-machine Dioxus UI emulator of the ArtNet bus side, targeting aarch64-apple-darwin). The bridge subscribes to MQTT topics under `DMX/#`, dispatches lighting commands to ArtNet universes and DMX arrays, and re-publishes state via `DMX/Active`, `DMX/Version`, and `DMX/LastError`/`DMX/Error`.

## Logging

Log levels follow the fleet policy at `~/Documents/Projects/Store/docs/guides/logging-policy.md`:
ERROR = a code change is warranted; WARN = someone must eventually act (zero per hour at idle) and always carries `kind = "<family>"`; INFO = lifecycle / designed degradations; per-frame DMX traffic → TRACE. External outages (broker down, ArtNet node unreachable) are `kind = "external_failure"` at WARN, never ERROR. The portable-core reference is the `logging-policy` user-level skill.

The bridge joins distributed traces via MQTT v5 `traceparent` user properties: inbound `Publish` packets are checked for a `traceparent` property and the handler span is re-parented before entering; outbound publishes (Active, Version, Error) stamp the current traceparent when a trace is active. The helpers live in `tracing_init::traceparent` (behind the `otel` feature). GELF and OTLP destinations are deploy-time config (env vars `LOG_DESTINATION`, `LOG_SERVER` or a `logging.toml` file).
