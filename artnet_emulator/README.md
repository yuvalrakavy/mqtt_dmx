# Art-Net emulator

Run the emulator instead of real DMX hardware during bridge development. Its UDP
receiver and browser UI retain their all-interface default. Set `--bind 127.0.0.1`,
`--port`, and `--web-port` to choose isolated endpoints; use `--config` for a fixture, or let received
universes appear automatically.

Use `dx serve`/`dx build` for the browser assets. For receiver-only probes with a
cargo-built server, set `DIOXUS_PUBLIC_PATH` to an existing empty directory; this
does not build the interactive browser assets.

## Logmon

The server initializes `tracing-init`. Pass `--logging-config /absolute/path/logging.toml`
to send telemetry to the intended Logmon domain. It does not discover the parent
bridge's logging configuration. Without this option, logs go to the console.

For Store lane t2, a local test configuration is:

```toml
[logging]
destination = "cg"
level = "info"
on_destination_error = "skip"

[logging.gelf]
address = "127.0.0.1:12203"
filter = "info,detail.artnet_rx=trace"
```

Confirm the lane/domain and its current port before running; other lanes have other
ports. No emulator telemetry needs to reach a house broker or collector.

Startup, receiver binding, configuration loading and newly discovered universes are
INFO. `detail.artnet_rx` is the packet detail family: TRACE records the source,
Art-Net address, sequence and actual channel values. Enable it when diagnosing output;
it is off by default so steady playback does not flood baseline logs. Malformed
packets are WARN with `kind=protocol_invalid`; receiver I/O failures use
`kind=external_failure`. Art-Net has no traceparent field: packet identity/time and
universe fields correlate bridge output with emulator receipt without inventing a
distributed trace link.
