//! Polls rumqttc's event loop in a task of its own (Store no-hang §14.3).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

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

/// The forward queue's depth, and whether its high-water WARN is standing.
#[derive(Default)]
struct Backlog {
    depth: AtomicUsize,
    high_since: Mutex<Option<Instant>>,
}

impl Backlog {
    fn pushed(&self) {
        let depth = self.depth.fetch_add(1, Ordering::SeqCst) + 1;
        if depth >= HIGH_WATER {
            // WAIT: mqtt-backlog-lock
            let mut high = self.high_since.lock().unwrap_or_else(|p| p.into_inner());
            if high.is_none() {
                *high = Some(Instant::now());
                warn!(kind = "mqtt_backlog_high", depth, "MQTT commands are arriving faster than the bridge handles them");
            }
        }
    }

    fn popped(&self) {
        let depth = self.depth.fetch_sub(1, Ordering::SeqCst) - 1;
        if depth <= LOW_WATER {
            // WAIT: mqtt-backlog-lock
            let mut high = self.high_since.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(since) = high.take() {
                info!(kind = "mqtt_backlog_drained", depth, lasted_ms = since.elapsed().as_millis() as u64, "MQTT command backlog drained");
            }
        }
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
    use super::{Backlog, HIGH_WATER, LOW_WATER};

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
