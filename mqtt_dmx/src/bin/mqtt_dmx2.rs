//! DMX v2 entry point. Legacy mqtt_dmx remains a separate binary and service.
use mqtt_dmx::service_v2::{bridge, DmxService};
use std::{path::PathBuf, process::ExitCode};
fn main() -> ExitCode {
    let bridge = bridge()
        .option(
            "startup-fallback",
            "Explicit startup fallback file; written only by SaveStartupFallback",
        )
        .from_cli();
    if let Some(code) = bridge.usage_exit() {
        return code;
    }
    let fallback = bridge
        .value("startup-fallback")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(format!(
                "mqtt_dmx2-{}-startup.json",
                bridge
                    .settings()
                    .instance
                    .as_deref()
                    .unwrap_or("unconfigured")
            ))
        });
    bridge.run_managed(DmxService::new(fallback))
}
