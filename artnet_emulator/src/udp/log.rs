use std::collections::VecDeque;

#[cfg(feature = "server")]
use std::sync::Arc;
#[cfg(feature = "server")]
use tokio::sync::{watch, RwLock};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub message: String,
}

#[cfg(feature = "server")]
pub struct ActivityLog {
    entries: VecDeque<LogEntry>,
    capacity: usize,
}

#[cfg(feature = "server")]
impl ActivityLog {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push(&mut self, message: String) {
        if self.entries.len() >= self.capacity {
            self.entries.pop_front();
        }
        let timestamp = chrono::Local::now().format("%H:%M:%S%.3f").to_string();
        self.entries.push_back(LogEntry { timestamp, message });
    }

    pub fn entries(&self) -> &VecDeque<LogEntry> {
        &self.entries
    }
}

#[cfg(feature = "server")]
#[derive(Clone)]
pub struct SharedLog {
    buffer: Arc<RwLock<ActivityLog>>,
    version_tx: Arc<watch::Sender<u64>>,
}

#[cfg(feature = "server")]
impl SharedLog {
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = watch::channel(0u64);
        SharedLog {
            buffer: Arc::new(RwLock::new(ActivityLog::new(capacity))),
            version_tx: Arc::new(tx),
        }
    }

    pub async fn push(&self, message: String) {
        let mut buf = self.buffer.write().await;
        buf.push(message);
        self.version_tx.send_modify(|v| *v += 1);
    }

    pub async fn wait_and_get(&self, last_version: u64) -> (u64, VecDeque<LogEntry>) {
        let mut rx = self.version_tx.subscribe();
        loop {
            if *rx.borrow() != last_version {
                break;
            }
            if rx.changed().await.is_err() {
                break;
            }
        }
        let version = *rx.borrow();
        let buf = self.buffer.read().await;
        (version, buf.entries().clone())
    }
}
