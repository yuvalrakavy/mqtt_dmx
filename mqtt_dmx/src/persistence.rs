//! The bridge's configuration on disk: read once at startup, and written by a writer of its own.
//!
//! Nothing on an async worker touches the filesystem (no-hang F1, Store 3b re-review X1). A call
//! into a stalled disk cannot be interrupted: a tokio worker inside one is lost to every task, and
//! dropping the runtime waits for it without limit. So the startup's read runs on a blocking
//! thread under a bound, and a save is posted — the latest content per file, replacing a save of
//! that file still waiting — to a writer task, which runs each write on a blocking thread, one at
//! a time. A write that passes its bound is a WARN, and the writer waits for it before the next:
//! a stalled disk holds one thread, and the saves posted meanwhile coalesce. Posting never waits;
//! nothing waits on the writer but the service's stop, within its bound.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Notify;
use tracing::{debug, error, info, warn};

use crate::defs::{DmxArray, EffectNodeDefinition, UniverseDefinition};

/// How long one write may take before it is a WARN.
const SAVE_WITHIN: Duration = Duration::from_secs(5);

/// The bridge's configuration: what the managers are given at startup, and what each MQTT session
/// starts from.
#[derive(Default, Clone)]
pub struct Config {
    pub universes: HashMap<Arc<str>, UniverseDefinition>,
    pub arrays: HashMap<Arc<str>, DmxArray>,
    pub effects: HashMap<Arc<str>, EffectNodeDefinition>,
    pub values: HashMap<Arc<str>, String>,
}

pub struct Persistence {
    storage_path: PathBuf,
    /// Held to read or change `State` alone: no I/O and no logging under it.
    state: Mutex<State>,
    /// Wakes the writer: a save was posted, or the persistence closed.
    wake: Notify,
}

#[derive(Default)]
struct State {
    /// The configuration as last saved, whether or not the disk has it yet.
    config: Config,
    /// Each file's latest content not yet written.
    pending: BTreeMap<&'static str, String>,
    /// No more saves: the writer ends once `pending` is written.
    closed: bool,
}

impl Persistence {
    pub fn new(storage_path: PathBuf) -> Self {
        Self {
            storage_path,
            state: Mutex::new(State::default()),
            wake: Notify::new(),
        }
    }

    /// Reads the saved configuration on a blocking thread, within `within`. Past it — a stalled
    /// disk — the bridge starts without it, with a WARN; the broker's retained configs restore it
    /// when the bridge subscribes, and the read is left behind (the runtime's shutdown does not
    /// wait for it).
    pub async fn load(&self, within: Duration) -> Config {
        let dir = self.storage_path.clone();
        let read = tokio::task::spawn_blocking(move || {
            if let Err(e) = fs::create_dir_all(&dir) {
                warn!(kind = "external_failure", path = %dir.display(), error = %e,
                      "failed to create storage directory");
            }
            read_config(&dir)
        });
        // WAIT: persistence-load
        let config = match tokio::time::timeout(within, read).await {
            Ok(Ok(config)) => config,
            Ok(Err(e)) => {
                error!(kind = "panic", error = %e, "reading the saved configuration panicked");
                Config::default()
            }
            Err(_) => {
                warn!(kind = "external_failure", path = %self.storage_path.display(),
                      bound_ms = within.as_millis() as u64,
                      "Reading the saved configuration timed out: starting without it, until the broker's retained configs arrive");
                Config::default()
            }
        };
        self.locked().config = config.clone();
        config
    }

    /// The configuration as last saved: a new MQTT session starts from it, not from the disk,
    /// which may not have it yet.
    pub fn config(&self) -> Config {
        self.locked().config.clone()
    }

    pub fn save_universes(&self, universes: &HashMap<Arc<str>, UniverseDefinition>) {
        self.save("universes.json", universes, |c| &mut c.universes);
    }

    pub fn save_arrays(&self, arrays: &HashMap<Arc<str>, DmxArray>) {
        self.save("arrays.json", arrays, |c| &mut c.arrays);
    }

    pub fn save_effects(&self, effects: &HashMap<Arc<str>, EffectNodeDefinition>) {
        self.save("effects.json", effects, |c| &mut c.effects);
    }

    pub fn save_values(&self, values: &HashMap<Arc<str>, String>) {
        self.save("values.json", values, |c| &mut c.values);
    }

    /// No more saves: the writer ends once what was posted is written.
    pub fn close(&self) {
        self.locked().closed = true;
        self.wake.notify_one();
    }

    /// The writer, for the service's life: writes each posted file on a blocking thread, one
    /// write at a time, and ends once closed and written.
    pub async fn write_saves(self: Arc<Self>) {
        let mut failing = Failing::default();
        loop {
            let next = {
                let mut state = self.locked();
                match state.pending.pop_first() {
                    Some(next) => Some(next),
                    None if state.closed => return,
                    None => None,
                }
            };
            let Some((file, json)) = next else {
                // A save posted since the look above left a permit, so this does not miss it.
                // WAIT: persistence-wake
                self.wake.notified().await;
                continue;
            };
            let path = self.storage_path.join(file);
            let dir = self.storage_path.clone();
            let mut write = tokio::task::spawn_blocking(move || write_file(&dir, file, &json));
            // WAIT: persistence-write
            let done = match tokio::time::timeout(SAVE_WITHIN, &mut write).await {
                Ok(done) => done,
                Err(_) => {
                    failing.failed(&path, &format!("the write did not finish within {} s", SAVE_WITHIN.as_secs()));
                    // One write at a time: the next waits for this one, and the saves posted
                    // meanwhile replace each other. Nothing waits on the writer but the stop,
                    // which leaves it behind past its own bound.
                    // WAIT: persistence-write-stalled
                    write.await
                }
            };
            match done {
                Ok(Ok(())) => failing.succeeded(),
                Ok(Err(e)) => failing.failed(&path, &e.to_string()),
                Err(e) => failing.failed(&path, &format!("the write panicked: {e}")),
            }
        }
    }

    /// Records `data` as `file`'s content and posts it to the writer. Never waits on the disk.
    fn save<T: Serialize + Clone>(
        &self,
        file: &'static str,
        data: &HashMap<Arc<str>, T>,
        slot: fn(&mut Config) -> &mut HashMap<Arc<str>, T>,
    ) {
        let json = match serde_json::to_string_pretty(data) {
            Ok(json) => json,
            Err(e) => {
                warn!(kind = "config_save_failed", file, error = %e, "failed to serialize config for persistence");
                return;
            }
        };
        {
            let mut state = self.locked();
            *slot(&mut state.config) = data.clone();
            state.pending.insert(file, json);
        }
        self.wake.notify_one();
    }

    fn locked(&self) -> std::sync::MutexGuard<'_, State> {
        // WAIT: persistence-lock
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// The writer's failures, as one episode: a WARN at the first, DEBUG for the rest, and an INFO
/// with how long it lasted when a save succeeds again.
#[derive(Default)]
struct Failing {
    since: Option<Instant>,
    failures: u64,
}

impl Failing {
    fn failed(&mut self, path: &Path, error: &str) {
        self.failures += 1;
        if self.since.is_none() {
            self.since = Some(Instant::now());
            warn!(kind = "config_save_failed", path = %path.display(), error,
                  "Saving the configuration failed: changes hold in memory until a save succeeds");
        } else {
            debug!(kind = "config_save_failed", path = %path.display(), error, failures = self.failures,
                   "Saving the configuration failed again");
        }
    }

    fn succeeded(&mut self) {
        if let Some(since) = self.since.take() {
            info!(kind = "config_save_recovered", down_for_ms = since.elapsed().as_millis() as u64,
                  failures = self.failures, "Saving the configuration works again");
        }
        self.failures = 0;
    }
}

/// Writes `json` as `file` in `dir`: a temporary file, then a rename over the old one. Blocking.
fn write_file(dir: &Path, file: &str, json: &str) -> std::io::Result<()> {
    let tmp = dir.join(format!(".{file}.tmp"));
    fs::write(&tmp, json)
        .map_err(|e| std::io::Error::new(e.kind(), format!("writing {}: {e}", tmp.display())))?;
    fs::rename(&tmp, dir.join(file))
        .map_err(|e| std::io::Error::new(e.kind(), format!("renaming {} to {file}: {e}", tmp.display())))
}

/// The configuration saved in `dir`. Blocking: for `load`'s thread, and the tests.
pub fn read_config(dir: &Path) -> Config {
    Config {
        universes: load_file(dir, "universes.json"),
        arrays: load_file(dir, "arrays.json"),
        effects: load_file(dir, "effects.json"),
        values: load_file(dir, "values.json"),
    }
}

fn load_file<T: for<'de> Deserialize<'de>>(dir: &Path, filename: &str) -> HashMap<Arc<str>, T> {
    let path = dir.join(filename);
    match fs::read_to_string(&path) {
        Ok(contents) => match serde_json::from_str(&contents) {
            Ok(data) => {
                info!("Loaded {} from {}", filename, path.display());
                data
            }
            Err(e) => {
                // Corrupt or schema-mismatched persisted file — log at ERROR
                // (the file format is ours; a parse failure warrants investigation).
                error!(kind = "decode_error", path = %path.display(), error = %e,
                       "failed to parse persisted config file");
                HashMap::new()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            info!("No persisted {} found at {}", filename, path.display());
            HashMap::new()
        }
        Err(e) => {
            warn!(kind = "external_failure", path = %path.display(), error = %e,
                  "failed to read persisted config file");
            HashMap::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Failing;
    use crate::test_log::capture;
    use std::path::Path;
    use tracing::Level;

    /// A disk that keeps failing is one episode in the log, however many saves meet it: a WARN at
    /// the first failure, DEBUG for the rest, and an INFO with how many there were once a save
    /// works again (the logging policy: zero WARNs an hour at idle, one per episode).
    #[test]
    fn a_failing_disk_is_one_warn_then_debug_and_an_info_when_saves_work_again() {
        let mut failing = Failing::default();
        let events = capture(|| {
            for _ in 0..3 {
                failing.failed(Path::new("dmx_config/universes.json"), "read-only file system");
            }
            failing.succeeded();
            failing.succeeded();
        });
        let warns: Vec<_> = events.iter().filter(|e| e.level <= Level::WARN).collect();
        assert_eq!(warns.len(), 1, "not one WARN for the episode: {warns:#?}");
        assert_eq!(warns[0].kind(), Some("config_save_failed"));
        let later = events.iter().filter(|e| e.level == Level::DEBUG && e.kind() == Some("config_save_failed")).count();
        assert_eq!(later, 2, "the later failures are not DEBUG: {events:#?}");
        let recovered: Vec<_> = events
            .iter()
            .filter(|e| e.level == Level::INFO && e.kind() == Some("config_save_recovered"))
            .collect();
        assert_eq!(recovered.len(), 1, "not one INFO when saves worked again: {events:#?}");
        assert_eq!(recovered[0].field("failures"), Some("3"), "the INFO does not count the episode's failures");
    }
}
