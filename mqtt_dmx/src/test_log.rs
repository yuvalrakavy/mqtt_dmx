//! Captures the log events emitted on the calling thread, for the tests that assert on logging:
//! a WARN once per episode, an INFO when it ends, nothing logged while a lock is held.

use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

/// One log event.
#[derive(Debug, Clone)]
pub struct Captured {
    pub level: Level,
    pub message: String,
    pub fields: Vec<(String, String)>,
    /// What the capture's probe answered as the event was emitted.
    pub probe: bool,
}

impl Captured {
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn kind(&self) -> Option<&str> {
        self.field("kind")
    }
}

/// Runs `f`, returning every event it emitted on this thread.
pub fn capture(f: impl FnOnce()) -> Vec<Captured> {
    capture_probing(|| false, f)
}

/// Like [`capture`], recording what `probe()` answers as each event is emitted — whether a lock
/// is held, say.
pub fn capture_probing(
    probe: impl Fn() -> bool + Send + Sync + 'static,
    f: impl FnOnce(),
) -> Vec<Captured> {
    let events = Arc::new(Mutex::new(Vec::new()));
    let layer = Capture {
        events: events.clone(),
        probe: Box::new(probe),
    };
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), f);
    let captured = events.lock().unwrap().clone();
    captured
}

struct Capture {
    events: Arc<Mutex<Vec<Captured>>>,
    probe: Box<dyn Fn() -> bool + Send + Sync>,
}

impl<S: Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        let probe = (self.probe)();
        self.events.lock().unwrap().push(Captured {
            level: *event.metadata().level(),
            message: fields.message,
            fields: fields.fields,
            probe,
        });
    }
}

#[derive(Default)]
struct Fields {
    message: String,
    fields: Vec<(String, String)>,
}

impl Fields {
    fn put(&mut self, field: &Field, value: String) {
        if field.name() == "message" {
            self.message = value;
        } else {
            self.fields.push((field.name().to_string(), value));
        }
    }
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.put(field, value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.put(field, format!("{value:?}"));
    }
}
