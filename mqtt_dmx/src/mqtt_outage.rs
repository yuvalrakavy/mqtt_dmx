//! The broker connection's outage, tracked across sessions (no-hang F2, the fleet logging policy).
//! An outage is one episode in the log: an INFO at its first failed attempt, DEBUG for every later
//! one, ONE WARN once it has lasted past a threshold — with its attempts and its length — and an
//! INFO with both when it ends.
//!
//! It ends on proof of life, not on a CONNACK: a connection has proved itself once it has lasted
//! `STABLE_AFTER`. A broker that takes the connection and drops it again — another instance with
//! this client id, a broker refusing the session — is one more failed attempt of the same outage,
//! which then reaches its WARN, instead of a recovery and a new outage every cycle. A session
//! cannot own this: each reconnect is a new one.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tracing::{debug, info, warn};

/// An outage this long is a WARN, once per outage.
const WARN_AFTER: Duration = Duration::from_secs(30);

/// A connection this old has proved itself. Twice the reconnect pacing (10 s), so two bridges
/// taking one client id from each other, each holding it until the other's next attempt, never
/// reach it; checked at the connection's traffic, which keep-alive pings bring every 5 s.
const STABLE_AFTER: Duration = Duration::from_secs(20);

/// Shared by `Service::mqtt`, which ends sessions, and each session's pump, which sees the broker
/// take a connection and keep it. Atomics only: neither waits on the other.
pub struct Outage {
    base: Instant,
    /// When the current outage began, in milliseconds since `base` plus one; 0 while there is none.
    down_since: AtomicU64,
    /// The current outage's failed attempts.
    attempts: AtomicU64,
    /// Whether this outage has had its WARN.
    warned: AtomicBool,
}

/// One connection, from its CONNACK: when it began, and whether it has proved itself.
pub struct Connection {
    at: u64,
    proven: bool,
}

impl Outage {
    pub fn new() -> Outage {
        Outage {
            base: Instant::now(),
            down_since: AtomicU64::new(0),
            attempts: AtomicU64::new(0),
            warned: AtomicBool::new(false),
        }
    }

    /// The broker took a connection (its CONNACK). Not a recovery yet: `alive` says when it is.
    pub fn connected(&self) -> Connection {
        self.connected_at(self.now())
    }

    /// Traffic on `connection`. Once it has lasted `STABLE_AFTER` it has proved itself, and the
    /// outage it ended, if any, is over: an INFO with its length (up to this connection) and its
    /// attempts.
    pub fn alive(&self, connection: &mut Connection) {
        self.alive_at(connection, self.now());
    }

    /// A session ended — its connection failed, was never made, or never proved itself — and the
    /// next is `retry_in` away: a failed attempt.
    pub fn session_ended(&self, retry_in: Duration) {
        self.session_ended_at(self.now(), retry_in);
    }

    fn now(&self) -> u64 {
        self.base.elapsed().as_millis() as u64 + 1
    }

    fn connected_at(&self, now: u64) -> Connection {
        Connection { at: now, proven: false }
    }

    fn alive_at(&self, connection: &mut Connection, now: u64) {
        if connection.proven || now.saturating_sub(connection.at) < STABLE_AFTER.as_millis() as u64 {
            return;
        }
        connection.proven = true;
        let since = self.down_since.load(Ordering::SeqCst);
        // No outage; or one that began after this connection did, which is not its to end (its
        // session ended while the pump took one last event).
        if since == 0 || since > connection.at {
            return;
        }
        if self
            .down_since
            .compare_exchange(since, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        let attempts = self.attempts.swap(0, Ordering::SeqCst);
        self.warned.store(false, Ordering::SeqCst);
        info!(
            kind = "external_recovered",
            down_for_ms = connection.at.saturating_sub(since),
            attempts,
            "MQTT broker connection restored"
        );
    }

    fn session_ended_at(&self, now: u64, retry_in: Duration) {
        let attempts = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
        let retry_in_s = retry_in.as_secs();
        // The outage began now, unless it already had.
        match self
            .down_since
            .compare_exchange(0, now, Ordering::SeqCst, Ordering::SeqCst)
        {
            Ok(_) => info!(kind = "connection_lost", retry_in_s, "MQTT session ended, reconnecting"),
            Err(since) => {
                let down_for_ms = now.saturating_sub(since);
                debug!(kind = "connection_lost", attempts, down_for_ms, retry_in_s, "MQTT reconnect attempt failed");
                if down_for_ms >= WARN_AFTER.as_millis() as u64 && !self.warned.swap(true, Ordering::SeqCst) {
                    warn!(
                        kind = "external_failure",
                        attempts,
                        down_for_ms,
                        "MQTT broker unreachable: no lasting connection for over 30 s"
                    );
                }
            }
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

    fn of_kind<'a>(
        events: &'a [crate::test_log::Captured],
        level: Level,
        kind: &str,
    ) -> Vec<&'a crate::test_log::Captured> {
        events
            .iter()
            .filter(|e| e.level == level && e.kind() == Some(kind))
            .collect()
    }

    /// An outage is one episode in the log (no-hang F2, Store 3b re-review X2): an INFO at its
    /// first failed attempt, DEBUG for every later one, and ONE WARN once it has lasted 30 s,
    /// carrying its attempts and its length.
    #[test]
    fn an_outage_is_one_info_then_debug_attempts_and_one_warn_past_30_s_with_its_attempts() {
        let outage = Outage::new();
        let events = capture(|| {
            // Down from 1 s, an attempt every 10 s for two minutes: 13 attempts.
            for at in (1_000..=121_000).step_by(10_000) {
                outage.session_ended_at(at, RETRY);
            }
        });

        let first = of_kind(&events, Level::INFO, "connection_lost");
        assert_eq!(first.len(), 1, "not one INFO, at the outage's first attempt: {events:#?}");
        let later = of_kind(&events, Level::DEBUG, "connection_lost");
        assert_eq!(later.len(), 12, "the later attempts are not DEBUG: {events:#?}");
        let warns: Vec<_> = events.iter().filter(|e| e.level <= Level::WARN).collect();
        assert_eq!(warns.len(), 1, "not one WARN for the outage: {warns:#?}");
        assert_eq!(warns[0].kind(), Some("external_failure"));
        assert_eq!(warns[0].field("down_for_ms"), Some("30000"), "the WARN came at the wrong attempt");
        assert_eq!(warns[0].field("attempts"), Some("4"), "the WARN does not carry the outage's attempts");
    }

    /// A broker that accepts the connection and drops it again — another instance with this
    /// client id, a broker that refuses the session — has not recovered: a CONNACK is not proof of
    /// life (no-hang F2). Each such cycle is one more failed attempt of the same outage, which
    /// reaches its WARN; the old code logged a recovery and a new outage per cycle, and never did.
    #[test]
    fn a_link_that_connects_and_drops_stays_one_outage() {
        let outage = Outage::new();
        let events = capture(|| {
            outage.session_ended_at(1_000, RETRY);
            // Every 10 s the broker takes the connection, answers on it (a SUBACK, say), and drops
            // it 2 s later, for two minutes.
            for at in (11_000..=121_000).step_by(10_000) {
                let mut connection = outage.connected_at(at);
                outage.alive_at(&mut connection, at + 1_000);
                outage.session_ended_at(at + 2_000, RETRY);
            }
        });

        let recovered = of_kind(&events, Level::INFO, "external_recovered");
        assert!(recovered.is_empty(), "a connection that dropped within seconds was logged as a recovery: {recovered:#?}");
        let first = of_kind(&events, Level::INFO, "connection_lost");
        assert_eq!(first.len(), 1, "a connection that dropped within seconds began a new outage: {events:#?}");
        let warns: Vec<_> = events.iter().filter(|e| e.level <= Level::WARN).collect();
        assert_eq!(warns.len(), 1, "the flapping outage never reached its one WARN: {warns:#?}");
    }

    /// The outage ends when a connection proves itself by lasting 20 s: one INFO, with the
    /// outage's length up to that connection and its failed attempts. A short outage has no WARN,
    /// and the next one starts with an INFO of its own.
    #[test]
    fn an_outage_ends_when_a_connection_proves_itself_with_its_length_and_attempts() {
        let outage = Outage::new();
        let events = capture(|| {
            // The first connection at startup ends no outage.
            let mut first = outage.connected_at(1);
            outage.alive_at(&mut first, 25_000);
            // Down from 30 s: three failed attempts, then a connection at 55 s that holds.
            for at in [30_000, 40_000, 50_000] {
                outage.session_ended_at(at, RETRY);
            }
            let mut held = outage.connected_at(55_000);
            outage.alive_at(&mut held, 60_000);
            outage.alive_at(&mut held, 75_000);
            outage.alive_at(&mut held, 80_000);
            // The next outage.
            outage.session_ended_at(90_000, RETRY);
        });

        let recovered = of_kind(&events, Level::INFO, "external_recovered");
        assert_eq!(recovered.len(), 1, "not one INFO when the connection proved itself: {events:#?}");
        assert_eq!(recovered[0].field("down_for_ms"), Some("25000"), "the INFO does not carry the outage's length");
        assert_eq!(recovered[0].field("attempts"), Some("3"), "the INFO does not carry the outage's attempts");
        assert_eq!(of_kind(&events, Level::INFO, "connection_lost").len(), 2, "the next outage did not begin with its INFO");
        assert!(events.iter().all(|e| e.level > Level::WARN), "a 25 s outage was a WARN: {events:#?}");
    }
}
