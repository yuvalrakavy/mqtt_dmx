//! Polls rumqttc's event loop in a task of its own (Store no-hang §14.3).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rumqttc::v5::mqttbytes::v5::{Packet, Publish};
use rumqttc::v5::{Event, EventLoop};
use tracing::{info, warn};

/// What the pump hands the subscriber.
pub enum PumpEvent {
    Publish(Publish),
    /// The connection failed; the subscriber ends and the session reconnects.
    Ended(String),
}

/// A forward queue past this many unread publishes is a WARN, once; back under `LOW_WATER`, an
/// INFO with how long it lasted. Nothing is dropped (Store no-hang §14.6, ruling 1).
const HIGH_WATER: usize = 1000;
const LOW_WATER: usize = 100;

/// The forward queue's depth, and whether its high-water WARN is standing. The flag's lock is held
/// to read or set it alone: the logging comes after it is released (tracing-init's console and
/// file writers are synchronous).
#[derive(Default)]
struct Backlog {
    depth: AtomicUsize,
    high_since: Mutex<Option<Instant>>,
}

impl Backlog {
    fn pushed(&self) {
        let depth = self.depth.fetch_add(1, Ordering::SeqCst) + 1;
        if depth >= HIGH_WATER && self.raise() {
            warn!(kind = "mqtt_backlog_high", depth, "MQTT commands are arriving faster than the bridge handles them");
        }
    }

    fn popped(&self) {
        let depth = self.depth.fetch_sub(1, Ordering::SeqCst) - 1;
        if depth <= LOW_WATER {
            if let Some(lasted) = self.clear() {
                info!(kind = "mqtt_backlog_drained", depth, lasted_ms = lasted.as_millis() as u64, "MQTT command backlog drained");
            }
        }
    }

    /// Raises the high-water flag; `true` if this call raised it.
    fn raise(&self) -> bool {
        // WAIT: mqtt-backlog-lock
        let mut high = self.high_since.lock().unwrap_or_else(|p| p.into_inner());
        if high.is_none() {
            *high = Some(Instant::now());
            true
        } else {
            false
        }
    }

    /// Clears the high-water flag; how long it stood, if it was raised.
    fn clear(&self) -> Option<Duration> {
        // WAIT: mqtt-backlog-lock
        let mut high = self.high_since.lock().unwrap_or_else(|p| p.into_inner());
        high.take().map(|since| since.elapsed())
    }
}

/// rumqttc's request channel drains only while the event loop is polled, so the task that polls
/// waits on nothing else — no publish, no subscribe, no bounded send — and forwards what arrives on
/// an unbounded queue. Dropping it stops the task, and with it the event loop: a publish still
/// waiting on the request channel then fails instead of waiting for good.
pub struct Pump {
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Pump {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The subscriber's end of the pump's queue.
pub struct Incoming {
    rx: tokio::sync::mpsc::UnboundedReceiver<PumpEvent>,
    backlog: Arc<Backlog>,
}

/// The session is over, and what its subscriber never read is discarded — when the publisher ends
/// first, the session aborts the subscriber with publishes still queued. They are counted in one
/// WARN, and a standing high-water WARN is closed: the next session has a backlog of its own.
impl Drop for Incoming {
    fn drop(&mut self) {
        self.rx.close();
        let mut discarded: u64 = 0;
        while let Ok(event) = self.rx.try_recv() {
            if matches!(event, PumpEvent::Publish(_)) {
                discarded += 1;
            }
        }
        if discarded > 0 {
            warn!(kind = "mqtt_commands_discarded", discarded, "MQTT commands discarded unhandled: their session ended");
        }
        if let Some(lasted) = self.backlog.clear() {
            info!(kind = "mqtt_backlog_drained", discarded, lasted_ms = lasted.as_millis() as u64,
                  "MQTT command backlog ended with its session");
        }
    }
}

impl Incoming {
    pub async fn recv(&mut self) -> Option<PumpEvent> {
        // WAIT: mqtt-pump-queue
        let event = self.rx.recv().await;
        if matches!(event, Some(PumpEvent::Publish(_))) {
            self.backlog.popped();
        }
        event
    }
}

impl Pump {
    pub fn start(mut events: EventLoop) -> (Pump, Incoming) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let backlog = Arc::new(Backlog::default());
        let pushed = backlog.clone();
        let task = tokio::spawn(async move {
            loop {
                // WAIT: mqtt-poll
                match events.poll().await {
                    Ok(Event::Incoming(Packet::Publish(publish))) => {
                        pushed.pushed();
                        if tx.send(PumpEvent::Publish(publish)).is_err() {
                            return; // the subscriber is gone
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        let _ = tx.send(PumpEvent::Ended(e.to_string()));
                        return;
                    }
                }
            }
        });
        (Pump { task }, Incoming { rx, backlog })
    }
}

#[cfg(test)]
mod tests {
    use super::{Backlog, Incoming, PumpEvent, HIGH_WATER, LOW_WATER};
    use crate::test_log::{capture, capture_probing};
    use rumqttc::v5::mqttbytes::{v5::Publish, QoS};
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use tracing::Level;

    /// The backlog's WARN and INFO are logged after its lock is released: tracing-init's console
    /// and file writers are synchronous, so a log line under the lock is a write to a file or a
    /// terminal the pump would wait on while holding it (Store no-hang 3b review, C-10).
    #[test]
    fn the_backlog_logs_nothing_while_it_holds_its_lock() {
        let backlog = Arc::new(Backlog::default());
        let probe = backlog.clone();
        let events = capture_probing(move || probe.high_since.try_lock().is_err(), || {
            for _ in 0..HIGH_WATER {
                backlog.pushed();
            }
            while backlog.depth.load(Ordering::SeqCst) > LOW_WATER {
                backlog.popped();
            }
        });
        let kinds: Vec<_> = events.iter().filter_map(|e| e.kind()).collect();
        assert_eq!(kinds, ["mqtt_backlog_high", "mqtt_backlog_drained"], "the backlog's WARN and INFO were not logged");
        let under_lock: Vec<_> = events.iter().filter(|e| e.probe).collect();
        assert!(under_lock.is_empty(), "logged while holding the backlog's lock: {under_lock:#?}");
    }

    /// When a session ends with publishes its subscriber never read — the publisher ended first, and
    /// the subscriber was aborted — they are discarded: one WARN counting them, and a standing
    /// high-water WARN closed by its INFO, which a new session's backlog would never send (Store
    /// no-hang 3b review, C-6).
    #[test]
    fn publishes_left_unread_when_a_session_ends_are_counted_and_its_high_water_warn_closed() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let backlog = Arc::new(Backlog::default());
        for _ in 0..HIGH_WATER {
            backlog.pushed();
            assert!(tx.send(PumpEvent::Publish(Publish::new("DMX/Command/On", QoS::AtMostOnce, "{}", None))).is_ok());
        }
        let incoming = Incoming { rx, backlog };
        let events = capture(|| drop(incoming));
        let discarded: Vec<_> = events
            .iter()
            .filter(|e| e.level == Level::WARN && e.kind() == Some("mqtt_commands_discarded"))
            .filter_map(|e| e.field("discarded"))
            .collect();
        assert_eq!(discarded, [HIGH_WATER.to_string()], "no WARN counting the publishes discarded at session end: {events:#?}");
        assert!(
            events.iter().any(|e| e.level == Level::INFO && e.kind() == Some("mqtt_backlog_drained")),
            "the standing high-water WARN was never closed: {events:#?}"
        );
    }

    /// The owner's overload ruling (no-hang §14.6): the forward queue drops nothing; past
    /// HIGH_WATER unread commands it raises its WARN once, and clears it back under LOW_WATER.
    #[test]
    fn a_backlog_past_high_water_is_flagged_once_and_cleared_when_it_drains() {
        let backlog = Backlog::default();
        for _ in 0..HIGH_WATER - 1 {
            backlog.pushed();
        }
        assert!(backlog.high_since.lock().unwrap().is_none(), "flagged below the high-water mark");
        backlog.pushed();
        let since = *backlog.high_since.lock().unwrap();
        assert!(since.is_some(), "not flagged at the high-water mark");
        backlog.pushed();
        assert_eq!(*backlog.high_since.lock().unwrap(), since, "flagged again while it stood");
        while backlog.depth.load(std::sync::atomic::Ordering::SeqCst) > LOW_WATER + 1 {
            backlog.popped();
        }
        assert!(backlog.high_since.lock().unwrap().is_some(), "cleared above the low-water mark");
        backlog.popped();
        assert!(backlog.high_since.lock().unwrap().is_none(), "not cleared at the low-water mark");
    }
}
