//! Complete published configuration. Labels remain authored references; UUIDs
//! identify objects independently of renames. Editor drafts never enter this type.
use crate::{canonicalize_json, decode_strict, sha256, Error, Id, Revision};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, net::SocketAddr};

pub const MAX_CONFIG_BYTES: usize = 128 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub schema: u8,
    pub history: Id,
    pub authority: Id,
    pub revision: Revision,
    pub expected_previous: Option<Revision>,
    pub content: Content,
    pub digest: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Content {
    pub universes: BTreeMap<Id, Universe>,
    pub arrays: BTreeMap<Id, Array>,
    pub effects: BTreeMap<Id, NamedEffect>,
    pub values: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Universe {
    pub name: String,
    pub description: String,
    pub endpoint: SocketAddr,
    pub net: u8,
    pub subnet: u8,
    pub universe: u8,
    pub channels: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Array {
    pub name: String,
    pub description: String,
    pub generation: Id,
    pub universe: String,
    pub lights: BTreeMap<String, String>,
    pub on: Option<Id>,
    pub off: Option<Id>,
    pub effects: BTreeMap<Id, NamedEffect>,
    pub defaults: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamedEffect {
    pub name: String,
    pub description: String,
    pub definition: Node,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Ticks {
    Number(u32),
    Parameter(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Node {
    Fade {
        lights: String,
        ticks: Ticks,
        target: String,
        no_dimming: bool,
    },
    Delay {
        ticks: Ticks,
    },
    Sequence {
        nodes: Vec<Node>,
    },
    Parallel {
        nodes: Vec<Node>,
    },
}

impl Content {
    pub fn digest(&self) -> Result<String, Error> {
        Ok(sha256(
            canonicalize_json(&serde_json::to_vec(self)?, MAX_CONFIG_BYTES)?.as_bytes(),
        ))
    }
}

impl Configuration {
    /// Identity of one publication, independent of bridge-assigned mapping
    /// generations and the digest derived from them. All authored and ordering
    /// fields remain part of the identity. Validate the supplied digest first.
    pub fn publication_fingerprint(&self) -> Result<String, Error> {
        self.encode()?;
        let mut value = serde_json::to_value(self)?;
        value.as_object_mut().unwrap().remove("digest");
        for array in value["content"]["arrays"]
            .as_object_mut()
            .unwrap()
            .values_mut()
        {
            array.as_object_mut().unwrap().remove("generation");
        }
        Ok(sha256(
            canonicalize_json(&serde_json::to_vec(&value)?, MAX_CONFIG_BYTES)?.as_bytes(),
        ))
    }

    pub fn encode(&self) -> Result<String, Error> {
        if self.schema != 1 || self.content.universes.len() > 32 || self.content.arrays.len() > 64 {
            return Err(Error::Invalid(
                "unsupported configuration schema or object count".into(),
            ));
        }
        if self.digest != self.content.digest()? {
            return Err(Error::Invalid("configuration digest mismatch".into()));
        }
        canonicalize_json(&serde_json::to_vec(self)?, MAX_CONFIG_BYTES)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let value: serde_json::Value = decode_strict(bytes, MAX_CONFIG_BYTES)?;
        if value.get("expected_previous").is_none() {
            return Err(Error::Invalid(
                "missing configuration expected_previous".into(),
            ));
        }
        let result: Self = serde_json::from_value(value)?;
        result.encode()?;
        Ok(result)
    }
}
