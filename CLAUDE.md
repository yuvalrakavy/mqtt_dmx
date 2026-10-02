# mqtt_dmx

Two crates: `mqtt_dmx/` (the bridge — cross-compiled for Raspberry Pi armv7 by default via `.cargo/config.toml`) and `artnet_emulator/` (a dev-machine Dioxus UI emulator of the ArtNet bus side, targeting aarch64-apple-darwin). The bridge subscribes to MQTT topics under `DMX/#`, dispatches lighting commands to ArtNet universes and DMX arrays, and re-publishes state via `DMX/Active`, `DMX/Version`, and `DMX/LastError`/`DMX/Error`. The broker argument is `host` or `host:port` (default 1883).

## Build and test

Tests run on the Mac, against an in-process fake broker (`mqtt-test-broker`) — never a real one: on a development Mac `localhost:1883` is the live local broker. Test universes use `disable_send`, so no test sends ArtNet.

```bash
cd mqtt_dmx
cargo test --target aarch64-apple-darwin
cargo clippy --target aarch64-apple-darwin --all-targets
```

## The MQTT loop (Store no-hang §14.3)

rumqttc's request channel drains only while its event loop is polled, so the task that polls waits on nothing else. `Pump` (`mqtt_pump.rs`) polls and forwards incoming publishes on an unbounded queue (a WARN with `kind = "mqtt_backlog_high"` past 1000 unread, an INFO when it drains; what is still queued when a session ends is discarded and counted in a WARN, `kind = "mqtt_commands_discarded"`); the subscriber handles them, the publisher publishes error reports, and the ArtNet manager hands its own error reports over without waiting (`Reporter`: with the queue full, the newest displaces the oldest — one WARN per episode, `kind = "error_report_dropped"`, and an INFO with the total once it drains). Every wait carries a `// WAIT: <row>` tag naming a row of `mqtt_dmx/docs/wait-registry.md`, which `tests/wait_registry.rs` checks (a tag sits on its own line above its statement: rustfmt moves a trailing one); the negative controls are `mqtt_dmx/docs/no-hang-3b-controls.toml`. SIGTERM (systemd's stop) and SIGINT stop the bridge through a bounded `Service::stop` (5 s, then a `shutdown_timeout` WARN), and the log is flushed before it exits (`tests/process.rs` runs the binary to check).

## Logging

Log levels follow the fleet policy at `~/Documents/Projects/Store/docs/guides/logging-policy.md`:
ERROR = a code change is warranted; WARN = someone must eventually act (zero per hour at idle) and always carries `kind = "<family>"`; INFO = lifecycle / designed degradations; per-frame DMX traffic → TRACE. External outages (broker down, ArtNet node unreachable) are `kind = "external_failure"` at WARN, never ERROR: a broker outage (`mqtt_outage.rs`, which outlives sessions) is an INFO per reconnect attempt, one WARN once it has lasted a minute, and an INFO (`kind = "external_recovered"`, `down_for_ms`) when the broker accepts a connection again. The portable-core reference is the `logging-policy` user-level skill.

The bridge joins distributed traces via MQTT v5 `traceparent` user properties: inbound `Publish` packets are checked for a `traceparent` property and the handler span is re-parented before entering; outbound publishes (Active, Version, Error) stamp the current traceparent when a trace is active. The helpers live in `tracing_init::traceparent` (behind the `otel` feature). GELF and OTLP destinations are deploy-time config (env vars `LOG_DESTINATION`, `LOG_SERVER` or a `logging.toml` file, found by an upward search from the working directory — on the Pi, `/home/yuval/logging.toml`, per `dmx_TLV.service`). `on_destination_error = skip` is pinned in code: no log destination can stop the bridge.
