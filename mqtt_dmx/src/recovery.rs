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
    pub revision: Revision,
    pub retired_ids: BTreeSet<Id>,
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
            revision: snapshot.revision,
            retired_ids: snapshot.retired_ids,
        })
    }
    /// Admission reserves the largest held-plane encoding and revision width a
    /// completion can produce. Frames therefore cannot exhaust the wire budget.
    pub fn encode_for_commit(&self) -> Result<String, Error> {
        let document = self.encode()?;
        let mut reserved = self.clone();
        reserved.revision = Revision(u64::MAX);
        let mut running = 0u64;
        for checkpoint in reserved.arrays.values_mut() {
            if checkpoint.player.status == crate::engine::Status::Running {
                running += 1;
                checkpoint.player.status = crate::engine::Status::Completed;
                for target in &mut checkpoint.player.held {
                    target.planes = dmx_protocol::ChannelPlanes {
                        dimmable: 255,
                        independent: 255,
                    };
                }
            }
        }
        if self.revision.0 > u64::MAX - running {
            return Err(Error::Invalid(
                "snapshot revision lacks completion headroom".into(),
            ));
        }
        let maximum_document_bytes = reserved.canonical()?.len();
        self.check_memory_budget(maximum_document_bytes)?;
        Ok(document)
    }
    fn check_memory_budget(&self, document_bytes: usize) -> Result<(), Error> {
        // Conservative admission units cover owned planes/maps, compiled/runtime
        // nodes and references, JSON decoding and bounded recovery copies. These
        // are policy charges, not a measurement of allocator resident memory.
        fn plan(effect: &crate::engine::Effect) -> usize {
            use crate::engine::Effect;
            1024 + match effect {
                Effect::Fade { targets, .. } => targets.len() * 1024,
                Effect::Sequence { nodes } | Effect::Parallel { nodes } => {
                    nodes.iter().map(plan).sum()
                }
                Effect::Delay { .. } => 0,
            }
        }
        let units = document_bytes * 32
            + self.configuration.encode()?.len() * 8
            + self
                .arrays
                .values()
                .map(|a| a.player.initial.len() * 1024 + plan(&a.player.effect))
                .sum::<usize>();
        if units > 16 * 1024 * 1024 {
            return Err(Error::Invalid(
                "installation exceeds 16MiB admission budget".into(),
            ));
        }
        Ok(())
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
            .chain(content.arrays.values().map(|a| &a.generation))
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
