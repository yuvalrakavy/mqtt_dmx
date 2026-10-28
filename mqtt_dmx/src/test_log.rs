//! Captures the log events emitted on the calling thread, for the tests that assert on logging:
//! a WARN once per episode, an INFO when it ends, nothing logged while a lock is held.
//!
//! One global subscriber, installed once, routes each event to the capture running on its thread,
//! if any. Not a scoped `with_default` per test: tracing caches each callsite's interest, and a
//! callsite first reached on a thread with no subscriber while another thread's scoped one was
//! being set up could be cached as "never", silently losing that test's events (seen as a flaky
//! miss of the backlog's WARN).

use std::cell::RefCell;
use std::sync::Once;

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

/// The capture running on a thread: its probe, and what it caught.
struct Sink {
    probe: Box<dyn Fn() -> bool>,
    events: Vec<Captured>,
}

thread_local! {
    static SINK: RefCell<Option<Sink>> = const { RefCell::new(None) };
}

/// Runs `f`, returning every event it emitted on this thread.
pub fn capture(f: impl FnOnce()) -> Vec<Captured> {
    capture_probing(|| false, f)
}

/// Like [`capture`], recording what `probe()` answers as each event is emitted — whether a lock
/// is held, say.
pub fn capture_probing(probe: impl Fn() -> bool + 'static, f: impl FnOnce()) -> Vec<Captured> {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        tracing::subscriber::set_global_default(tracing_subscriber::registry().with(Router))
            .expect("no other global subscriber in the test binary");
    });
    // A callsite another thread was registering as the subscriber went in may have cached its
    // interest from before it: recompute every callsite's now.
    tracing::callsite::rebuild_interest_cache();
    SINK.with(|sink| {
        *sink.borrow_mut() = Some(Sink {
            probe: Box::new(probe),
            events: Vec::new(),
        })
    });
    f();
    SINK.with(|sink| sink.borrow_mut().take())
        .map(|sink| sink.events)
        .unwrap_or_default()
}

/// Hands each event to its thread's capture.
struct Router;

impl<S: Subscriber> Layer<S> for Router {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        SINK.with(|sink| {
            // `try_borrow_mut`: an event logged from inside a probe is not captured.
            if let Ok(mut sink) = sink.try_borrow_mut() {
                if let Some(sink) = sink.as_mut() {
                    let mut fields = Fields::default();
                    event.record(&mut fields);
                    let probe = (sink.probe)();
                    sink.events.push(Captured {
                        level: *event.metadata().level(),
                        message: fields.message,
                        fields: fields.fields,
                        probe,
                    });
                }
            }
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
