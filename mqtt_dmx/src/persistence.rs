use log::{error, info};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use crate::defs::{DmxArray, EffectNodeDefinition, UniverseDefinition};

pub struct Persistence {
    storage_path: PathBuf,
}

impl Persistence {
    pub fn new(storage_path: PathBuf) -> Self {
        Self { storage_path }
    }

    pub fn ensure_directory(&self) -> Result<(), std::io::Error> {
        fs::create_dir_all(&self.storage_path)
    }

    fn file_path(&self, filename: &str) -> PathBuf {
        self.storage_path.join(filename)
    }

    fn load_file<T: for<'de> Deserialize<'de>>(&self, filename: &str) -> HashMap<Arc<str>, T> {
        let path = self.file_path(filename);
        match fs::read_to_string(&path) {
            Ok(contents) => match serde_json::from_str(&contents) {
                Ok(data) => {
                    info!("Loaded {} from {}", filename, path.display());
                    data
                }
                Err(e) => {
                    error!("Failed to parse {}: {}", path.display(), e);
                    HashMap::new()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                info!("No persisted {} found at {}", filename, path.display());
                HashMap::new()
            }
            Err(e) => {
                error!("Failed to read {}: {}", path.display(), e);
                HashMap::new()
            }
        }
    }

    fn save_file<T: Serialize>(&self, filename: &str, data: &HashMap<Arc<str>, T>) {
        let path = self.file_path(filename);
        let tmp_path = self.file_path(&format!(".{}.tmp", filename));
        match serde_json::to_string_pretty(data) {
            Ok(json) => {
                if let Err(e) = fs::write(&tmp_path, &json) {
                    error!("Failed to write {}: {}", tmp_path.display(), e);
                    return;
                }
                if let Err(e) = fs::rename(&tmp_path, &path) {
                    error!("Failed to rename {} to {}: {}", tmp_path.display(), path.display(), e);
                }
            }
            Err(e) => {
                error!("Failed to serialize {}: {}", path.display(), e);
            }
        }
    }

    // --- Universes ---

    pub fn load_universes(&self) -> HashMap<Arc<str>, UniverseDefinition> {
        self.load_file("universes.json")
    }

    pub fn save_universes(&self, universes: &HashMap<Arc<str>, UniverseDefinition>) {
        self.save_file("universes.json", universes);
    }

    // --- Arrays ---

    pub fn load_arrays(&self) -> HashMap<Arc<str>, DmxArray> {
        self.load_file("arrays.json")
    }

    pub fn save_arrays(&self, arrays: &HashMap<Arc<str>, DmxArray>) {
        self.save_file("arrays.json", arrays);
    }

    // --- Effects ---

    pub fn load_effects(&self) -> HashMap<Arc<str>, EffectNodeDefinition> {
        self.load_file("effects.json")
    }

    pub fn save_effects(&self, effects: &HashMap<Arc<str>, EffectNodeDefinition>) {
        self.save_file("effects.json", effects);
    }

    // --- Values ---

    pub fn load_values(&self) -> HashMap<Arc<str>, String> {
        self.load_file("values.json")
    }

    pub fn save_values(&self, values: &HashMap<Arc<str>, String>) {
        self.save_file("values.json", values);
    }
}
