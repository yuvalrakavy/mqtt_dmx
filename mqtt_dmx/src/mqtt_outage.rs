//! The broker connection's outage, tracked across sessions (the fleet logging policy: an outage is
//! an INFO for each attempt, ONE WARN once it has lasted past a threshold, and an INFO with how
//! long it lasted when it ends). A session cannot own this: each reconnect is a new one.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tracing::{info, warn};

/// An outage this long is a WARN, once per outage.
const WARN_AFTER: Duration = Duration::from_secs(60);

/// Shared by `Service::mqtt`, which ends sessions, and each session's pump, which sees the broker
/// accept a connection. Atomics only: neither waits on the other.
pub struct Outage {
    base: Instant,
    /// When the current outage began, in milliseconds since `base` plus one; 0 while connected.
    down_since: AtomicU64,
    /// Whether this outage has had its WARN.
    warned: AtomicBool,
}

impl Outage {
    pub fn new() -> Outage {
        Outage {
            base: Instant::now(),
            down_since: AtomicU64::new(0),
            warned: AtomicBool::new(false),
        }
    }

    /// The broker accepted a connection (its CONNACK): an outage, if there was one, is over.
    pub fn connected(&self) {
        self.connected_at(self.now());
    }

    /// A session ended — its connection failed, or was never made — and the next is `retry_in` away.
    pub fn session_ended(&self, retry_in: Duration) {
        self.session_ended_at(self.now(), retry_in);
    }

    fn now(&self) -> u64 {
        self.base.elapsed().as_millis() as u64 + 1
    }

    fn connected_at(&self, now: u64) {
        let since = self.down_since.swap(0, Ordering::SeqCst);
        self.warned.store(false, Ordering::SeqCst);
        if since != 0 {
            info!(
                kind = "external_recovered",
                down_for_ms = now.saturating_sub(since),
                "MQTT broker connection restored"
            );
        }
    }

    fn session_ended_at(&self, now: u64, retry_in: Duration) {
        // The outage began now, unless it already had.
        let since = self
            .down_since
            .compare_exchange(0, now, Ordering::SeqCst, Ordering::SeqCst)
            .err()
            .unwrap_or(now);
        let down_for_ms = now.saturating_sub(since);
        info!(
            kind = "connection_lost",
            down_for_ms,
            retry_in_s = retry_in.as_secs(),
            "MQTT session ended, reconnecting"
        );
        if down_for_ms >= WARN_AFTER.as_millis() as u64 && !self.warned.swap(true, Ordering::SeqCst)
        {
            warn!(
                kind = "external_failure",
                down_for_ms, "MQTT broker unreachable: no connection for over a minute"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Outage;
    use crate::test_log::capture;
    use std::time::Duration;
    use tracing::Level;

    const RETRY: Duration = Duration::from_secs(10);

    /// An outage is an INFO per attempt, one WARN once it has lasted a minute, and an INFO with
    /// its length when the broker is back; a short outage is never a WARN (Store no-hang 3b review,
    /// C-5).
    #[test]
    fn an_outage_is_info_per_attempt_one_warn_past_a_minute_and_an_info_with_its_length_at_the_end()
    {
        let outage = Outage::new();
        let events = capture(|| {
            // The first connection at startup ends no outage.
            outage.connected_at(1);
            // Down from 1 s, an attempt every 10 s for two minutes.
            for at in (1_000..=121_000).step_by(10_000) {
                outage.session_ended_at(at, RETRY);
            }
            outage.connected_at(125_000);
            // A short outage.
            outage.session_ended_at(200_000, RETRY);
            outage.connected_at(205_000);
        });

        let attempts = events
            .iter()
            .filter(|e| e.level == Level::INFO && e.kind() == Some("connection_lost"))
            .count();
        assert_eq!(attempts, 14, "not one INFO per failed attempt");
        let recovered: Vec<_> = events
            .iter()
            .filter(|e| e.level == Level::INFO && e.kind() == Some("external_recovered"))
            .filter_map(|e| e.field("down_for_ms"))
            .collect();
        assert_eq!(
            recovered,
            ["124000", "5000"],
            "no INFO with each outage's length when it ended"
        );
        let warns: Vec<_> = events.iter().filter(|e| e.level <= Level::WARN).collect();
        assert_eq!(
            warns.len(),
            1,
            "not one WARN for the long outage alone: {warns:#?}"
        );
        assert_eq!(warns[0].kind(), Some("external_failure"));
        assert_eq!(
            warns[0].field("down_for_ms"),
            Some("60000"),
            "the WARN came at the wrong attempt"
        );
    }
}
