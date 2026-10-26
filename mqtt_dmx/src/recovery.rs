//! Installation recovery documents. Validation constructs a complete replacement
//! off to the side; the service installs it only after every array is accepted.
use crate::{
    compiler::CompiledConfiguration,
    ordered_array::{Checkpoint, OrderedArray},
};
use dmx_protocol::{
    canonicalize_json, configuration::Configuration, decode_strict, sha256, Error, Id, Playback,
    Revision, MAX_RECOVERY_BYTES,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub schema: u8,
    pub history: Id,
    pub authority: Id,
    pub revision: Revision,
    pub configuration: Configuration,
    pub arrays: BTreeMap<Id, Checkpoint>,
    pub retired_ids: BTreeSet<Id>,
}

pub struct RestoredInstallation {
    pub configuration: CompiledConfiguration,
    pub arrays: BTreeMap<Id, OrderedArray>,
    pub document: String,
    pub digest: String,
}

impl Snapshot {
    pub fn encode(&self) -> Result<String, Error> {
        let document = self.canonical()?;
        self.validate()?;
        Ok(document)
    }
    pub fn decode(bytes: &[u8]) -> Result<RestoredInstallation, Error> {
        let snapshot: Self = decode_strict(bytes, MAX_RECOVERY_BYTES)?;
        let document = snapshot.canonical()?;
        if document.as_bytes() != bytes {
            return Err(Error::Invalid(
                "recovery document must use canonical encoding".into(),
            ));
        }
        let (configuration, arrays) = snapshot.validate()?;
        Ok(RestoredInstallation {
            configuration,
            arrays,
            digest: sha256(document.as_bytes()),
            document,
        })
    }
    fn canonical(&self) -> Result<String, Error> {
        let document = canonicalize_json(&serde_json::to_vec(self)?, MAX_RECOVERY_BYTES)?;
        // Snapshot is carried by a STRING in the reserved retained state record.
        if serde_json::to_vec(&serde_json::json!({"snapshot":document}))?.len() > MAX_RECOVERY_BYTES
        {
            return Err(Error::Invalid(
                "recovery exceeds 256KiB carrier limit".into(),
            ));
        }
        Ok(document)
    }
    fn validate(&self) -> Result<(CompiledConfiguration, BTreeMap<Id, OrderedArray>), Error> {
        if self.schema != 1
            || self.history != self.configuration.history
            || self.authority != self.configuration.authority
            || self.retired_ids.len() > 1024
            || self.arrays.len() != self.configuration.content.arrays.len()
        {
            return Err(Error::Invalid(
                "inconsistent installation recovery header/counts".into(),
            ));
        }
        let content = &self.configuration.content;
        for id in content
            .universes
            .keys()
            .chain(content.arrays.keys())
            .chain(content.effects.keys())
            .chain(content.arrays.values().flat_map(|a| a.effects.keys()))
        {
            if self.retired_ids.contains(id) {
                return Err(Error::Invalid("retired object identity reused".into()));
            }
        }
        let configuration = CompiledConfiguration::compile(self.configuration.clone())?;
        let mut arrays = BTreeMap::new();
        for (id, definition) in &content.arrays {
            let checkpoint = self
                .arrays
                .get(id)
                .ok_or_else(|| Error::Invalid("missing array recovery record".into()))?;
            let playback = Playback::decode(checkpoint.playback.as_bytes())?;
            if playback.history != self.history
                || playback.authority != self.authority
                || playback.generation != definition.generation
            {
                return Err(Error::Invalid(
                    "array recovery identity differs from installation mapping".into(),
                ));
            }
            arrays.insert(
                id.clone(),
                OrderedArray::restore(configuration.ownership()[id].clone(), checkpoint.clone())?,
            );
        }
        Ok((configuration, arrays))
    }
}
