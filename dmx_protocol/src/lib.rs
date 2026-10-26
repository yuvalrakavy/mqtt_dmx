//! Pure protocol and recovery primitives shared by the DMX bridge and Store.
//! External playback documents enter through [`Playback::decode`]. There is no
//! hardware, MQTT, clock, or filesystem access in this crate.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

mod strict_json;

pub const MAX_PLAYBACK_BYTES: usize = 16 * 1024;
pub const MAX_RECOVERY_BYTES: usize = 256 * 1024;

/// Bounded, duplicate-key-rejecting JSON entry point for recovery/file envelopes.
pub fn decode_strict<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
    limit: usize,
) -> Result<T, Error> {
    if bytes.len() > limit.min(1024 * 1024) {
        return Err(Error::Invalid(
            "JSON document exceeds its byte limit".into(),
        ));
    }
    let strict_json::StrictValue(value) = serde_json::from_slice(bytes)?;
    fn check_depth(value: &serde_json::Value, depth: usize) -> Result<(), Error> {
        if depth > 32 {
            return Err(Error::Invalid("JSON nesting exceeds 32 levels".into()));
        }
        match value {
            serde_json::Value::Object(values) => {
                for value in values.values() {
                    check_depth(value, depth + 1)?;
                }
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    check_depth(value, depth + 1)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    check_depth(&value, 0)?;
    Ok(serde_json::from_value(value)?)
}

/// Canonicalize a bounded JSON document without losing duplicate-key evidence.
pub fn canonicalize_json(bytes: &[u8], limit: usize) -> Result<String, Error> {
    let value: serde_json::Value = decode_strict(bytes, limit)?;
    let text = serde_json::to_string(&strict_json::sorted(value))?;
    if text.len() > limit {
        return Err(Error::Invalid("canonical JSON exceeds byte limit".into()));
    }
    Ok(text)
}

pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Id(String);

impl TryFrom<String> for Id {
    type Error = Error;

    fn try_from(value: String) -> Result<Self, Error> {
        let parsed = uuid::Uuid::parse_str(&value)
            .map_err(|_| Error::Invalid("identity must be a canonical UUID".into()))?;
        if parsed.to_string() != value {
            return Err(Error::Invalid("identity must be a canonical UUID".into()));
        }
        Ok(Self(value))
    }
}

impl From<Id> for String {
    fn from(value: Id) -> String {
        value.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Revision(pub u64);

impl TryFrom<String> for Revision {
    type Error = Error;
    fn try_from(value: String) -> Result<Self, Error> {
        let parsed = value
            .parse::<u64>()
            .map_err(|_| Error::Invalid("revision must be canonical u64 text".into()))?;
        if parsed.to_string() != value {
            return Err(Error::Invalid("revision must be canonical u64 text".into()));
        }
        Ok(Self(parsed))
    }
}

impl From<Revision> for String {
    fn from(value: Revision) -> String {
        value.0.to_string()
    }
}

impl Revision {
    pub fn next(self) -> Result<Self, Error> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or_else(|| Error::Invalid("revision exhausted; initialize a new history".into()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct Level(u16);

impl TryFrom<u16> for Level {
    type Error = Error;
    fn try_from(value: u16) -> Result<Self, Error> {
        if value > 1000 {
            return Err(Error::Invalid("level must be between 0 and 1000".into()));
        }
        Ok(Self(value))
    }
}

impl From<Level> for u16 {
    fn from(value: Level) -> u16 {
        value.0
    }
}

impl Level {
    pub const OFF: Self = Self(0);
    pub const FULL: Self = Self(1000);
    pub fn is_on(self) -> bool {
        self.0 != 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Playback {
    pub schema: u8,
    pub history: Id,
    pub authority: Id,
    pub revision: Revision,
    pub generation: Id,
    pub level: Level,
    pub effect: Option<Id>,
    pub parameters: BTreeMap<String, String>,
    pub off_effect: Option<Id>,
    pub fade_ms: u64,
}

impl Playback {
    pub fn encode(&self) -> Result<String, Error> {
        self.validate()?;
        let value = strict_json::sorted(serde_json::to_value(self)?);
        let text = serde_json::to_string(&value)?;
        // The carrier is a JSON string, so its escaping counts toward the
        // property payload limit too, not only toward the document's length.
        if serde_json::to_vec(&text)?.len() > MAX_PLAYBACK_BYTES {
            return Err(Error::Invalid("playback exceeds 16KiB wire limit".into()));
        }
        Ok(text)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_PLAYBACK_BYTES {
            return Err(Error::Invalid("playback exceeds 16KiB input limit".into()));
        }
        let strict_json::StrictValue(value) = serde_json::from_slice(bytes)?;
        // A complete request must spell optional selections as null rather
        // than allowing a partial document to reset them by omission.
        for key in ["effect", "off_effect"] {
            if value.get(key).is_none() {
                return Err(Error::Invalid(format!("missing playback field: {key}")));
            }
        }
        let playback: Self = serde_json::from_value(value)?;
        playback.encode()?;
        Ok(playback)
    }
    pub fn fingerprint(&self) -> Result<String, Error> {
        Ok(sha256(self.encode()?.as_bytes()))
    }
    fn validate(&self) -> Result<(), Error> {
        if self.schema != 1 {
            return Err(Error::Invalid("unsupported playback schema".into()));
        }
        for (name, value) in &self.parameters {
            if name.is_empty() || name.len() > 128 || value.len() > 1024 {
                return Err(Error::Invalid(
                    "parameter name/value exceeds its byte limit".into(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationStamp {
    pub revision: Revision,
    pub fingerprint: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    New,
    Duplicate,
}

pub fn classify_operation(
    current: Option<&OperationStamp>,
    candidate: &OperationStamp,
) -> Result<Admission, Error> {
    match current {
        Some(current) if candidate.revision < current.revision => {
            Err(Error::Invalid("operation superseded".into()))
        }
        Some(current) if candidate.revision == current.revision => {
            if candidate.fingerprint == current.fingerprint {
                Ok(Admission::Duplicate)
            } else {
                Err(Error::Invalid(
                    "conflicting contents at the same revision".into(),
                ))
            }
        }
        _ => Ok(Admission::New),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelPlanes {
    pub dimmable: u8,
    pub independent: u8,
}

impl ChannelPlanes {
    pub fn render(self, level: Level) -> u8 {
        if !level.is_on() {
            return 0;
        }
        let dimmed = (u32::from(self.dimmable) * u32::from(level.0) + 500) / 1000;
        (dimmed + u32::from(self.independent)).min(255) as u8
    }
}
