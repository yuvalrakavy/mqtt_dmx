//! Single-owner installation commits. Preparation acts on cloned candidate state;
//! snapshot admission and the original deadline are checked before the live swap.
use crate::{
    compiler::CompiledConfiguration,
    engine::{Channel, Effect},
    ordered_array::{Command, OrderedArray, Receipt},
    recovery::Snapshot,
};
use dmx_protocol::{configuration::Configuration, Error, Id, Level, Playback, Revision};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Instant,
};
mod preparation;
pub use preparation::{Edit, EditOutcome, Preparation, PreparedEdit};

#[derive(Clone)]
pub struct Installation {
    configuration: CompiledConfiguration,
    arrays: BTreeMap<Id, OrderedArray>,
    revision: Revision,
    document: Arc<str>,
    retired_ids: BTreeSet<Id>,
    /// Process-local edit identity. Ticks/completions do not retire preparation;
    /// accepted requests and replacement installations do.
    edit_epoch: Arc<()>,
}

pub struct ConfigurationCommit {
    pub revision: Revision,
    /// The output adapter clears these physical addresses at the activation tick.
    pub released: Vec<Channel>,
}

#[derive(serde::Serialize)]
pub struct ArrayState {
    pub playback: String,
    pub status: crate::engine::Status,
    pub execution: Id,
    pub operation_revision: Revision,
}

impl Installation {
    pub fn initialize(
        configuration: Configuration,
        executions: BTreeMap<Id, Id>,
    ) -> Result<Self, Error> {
        if configuration.expected_previous.is_some()
            || executions.len() != configuration.content.arrays.len()
            || executions.values().collect::<BTreeSet<_>>().len() != executions.len()
        {
            return Err(Error::Invalid(
                "invalid initial configuration or execution identities".into(),
            ));
        }
        let configuration = CompiledConfiguration::compile(configuration)?;
        let mut arrays = BTreeMap::new();
        for (id, definition) in &configuration.content().arrays {
            let playback = Playback {
                schema: 1,
                history: configuration.configuration().history.clone(),
                authority: configuration.configuration().authority.clone(),
                revision: Revision(0),
                generation: definition.generation.clone(),
                level: Level::OFF,
                effect: None,
                parameters: BTreeMap::new(),
                off_effect: None,
                fade_ms: 0,
            };
            let execution = executions
                .get(id)
                .ok_or_else(|| Error::Invalid("missing initial execution identity".into()))?;
            arrays.insert(
                id.clone(),
                OrderedArray::new(
                    configuration.ownership()[id].clone(),
                    playback,
                    Effect::Delay { ticks: 0 },
                    execution.clone(),
                )?,
            );
        }
        let mut installation = Self {
            configuration,
            arrays,
            revision: Revision(1),
            document: Arc::from(""),
            retired_ids: BTreeSet::new(),
            edit_epoch: Arc::new(()),
        };
        installation.document = Arc::from(
            installation
                .capture(None, installation.revision)?
                .encode_validated_for_commit()?,
        );
        Ok(installation)
    }
    pub fn restore(snapshot: &[u8]) -> Result<Self, Error> {
        let restored = Snapshot::decode(snapshot)?;
        let installation = Self {
            configuration: restored.configuration,
            arrays: restored.arrays,
            revision: restored.revision,
            document: Arc::from(restored.document),
            retired_ids: restored.retired_ids,
            edit_epoch: Arc::new(()),
        };
        installation
            .capture(None, installation.revision)?
            .encode_validated_for_commit()?;
        Ok(installation)
    }
    pub fn document(&self) -> Arc<str> {
        self.document.clone()
    }
    /// Prepare broker adoption without restarting matching live executions.
    /// Matching local completions keep their final planes and publish one cut
    /// above both revisions. The owner decides whether adoption is permitted.
    pub fn preserve_running_from(mut self, live: &Self) -> Result<Self, Error> {
        if self.configuration().history != live.configuration().history
            || self.configuration().authority != live.configuration().authority
        {
            return Err(Error::Invalid(
                "continuity requires matching history and authority".into(),
            ));
        }
        let mut promoted = false;
        for (id, selected) in &mut self.arrays {
            if let Some(previous) = live.arrays.get(id) {
                promoted |= selected.preserve_running_from(previous)?;
            }
        }
        if promoted {
            self.revision = std::cmp::max(self.revision, live.revision).next()?;
            self.document = Arc::from(
                self.capture(None, self.revision)?
                    .encode_validated_for_commit()?,
            );
        }
        Ok(self)
    }
    pub fn revision(&self) -> Revision {
        self.revision
    }
    pub fn configuration(&self) -> &Configuration {
        self.configuration.configuration()
    }
    pub fn validate_configuration(&self, configuration: &Configuration) -> Result<(), Error> {
        let current = self.configuration();
        if configuration.history != current.history || configuration.authority != current.authority
        {
            return Err(Error::Invalid(
                "configuration history/authority is fenced".into(),
            ));
        }
        let encoded = configuration.encode()?;
        if configuration.revision < current.revision {
            return Err(Error::Invalid("configuration superseded".into()));
        }
        if configuration.revision == current.revision {
            if encoded != current.encode()? {
                return Err(Error::Invalid(
                    "conflicting configuration at equal revision".into(),
                ));
            }
        } else if configuration.expected_previous != Some(current.revision) {
            return Err(Error::Invalid(
                "configuration expected previous revision differs".into(),
            ));
        }
        Ok(())
    }
    /// Fixed physical ownership used by the output adapter; never resolve labels
    /// or acquire new channels while rendering a frame.
    pub fn channels(&self) -> impl Iterator<Item = (&Id, &Channel)> {
        self.configuration
            .ownership()
            .iter()
            .flat_map(|(array, channels)| channels.iter().map(move |channel| (array, channel)))
    }
    pub fn configure(
        &mut self,
        configuration: Configuration,
        executions: BTreeMap<Id, Id>,
        deadline: Option<Instant>,
        clock: impl Fn() -> Instant,
    ) -> Result<ConfigurationCommit, Error> {
        check_deadline(deadline, clock())?;
        let current = self.configuration();
        if configuration.history != current.history || configuration.authority != current.authority
        {
            return Err(Error::Invalid(
                "configuration history/authority is fenced".into(),
            ));
        }
        let encoded = configuration.encode()?;
        if configuration.revision < current.revision {
            return Err(Error::Invalid("configuration superseded".into()));
        }
        if configuration.revision == current.revision {
            if encoded != current.encode()? {
                return Err(Error::Invalid(
                    "conflicting configuration at equal revision".into(),
                ));
            }
            check_deadline(deadline, clock())?;
            return Ok(ConfigurationCommit {
                revision: current.revision,
                released: vec![],
            });
        }
        if configuration.expected_previous != Some(current.revision) {
            return Err(Error::Invalid(
                "configuration expected previous revision differs".into(),
            ));
        }
        let compiled = CompiledConfiguration::compile(configuration)?;
        let mut retired_ids = self.retired_ids.clone();
        let previous_ids = object_ids(self.configuration());
        let next_ids = object_ids(compiled.configuration());
        let previous_kinds = identity_kinds(self.configuration());
        for (id, kind) in identity_kinds(compiled.configuration()) {
            if previous_kinds
                .get(&id)
                .is_some_and(|previous| previous != &kind)
            {
                return Err(Error::Invalid(
                    "configuration cannot change an identity's object kind".into(),
                ));
            }
        }
        retired_ids.extend(previous_ids.difference(&next_ids).cloned());
        // A generation belongs to one array. Retire it when that array changes
        // or disappears, even if another proposed array would keep its UUID live.
        for (id, previous) in &self.configuration.content().arrays {
            if compiled
                .content()
                .arrays
                .get(id)
                .map(|array| &array.generation)
                != Some(&previous.generation)
            {
                retired_ids.insert(previous.generation.clone());
            }
        }
        if retired_ids.len() > 1024 || next_ids.iter().any(|id| retired_ids.contains(id)) {
            return Err(Error::Invalid(
                "configuration reuses retired identity or exceeds retirement budget".into(),
            ));
        }
        for (id, old) in &self.arrays {
            let mapping_changed =
                compiled.ownership().get(id) != self.configuration.ownership().get(id);
            if mapping_changed && !old.completed_off() {
                return Err(Error::Invalid("remap/delete requires completed Off".into()));
            }
        }
        let mut arrays = BTreeMap::new();
        let mut needed_executions = BTreeSet::new();
        for (id, definition) in &compiled.content().arrays {
            let mapping_changed =
                compiled.ownership().get(id) != self.configuration.ownership().get(id);
            if let Some(previous) = self.configuration.content().arrays.get(id) {
                if (definition.generation == previous.generation) == mapping_changed {
                    return Err(Error::Invalid(
                        "mapping changes must rotate generation; unchanged mappings retain it"
                            .into(),
                    ));
                }
            }
            if !mapping_changed {
                arrays.insert(id.clone(), self.array(id)?.clone());
                continue;
            }
            needed_executions.insert(id.clone());
            let execution = executions
                .get(id)
                .ok_or_else(|| Error::Invalid("new mapping lacks execution identity".into()))?;
            if self
                .arrays
                .values()
                .any(|a| a.receipt().execution == *execution)
            {
                return Err(Error::Invalid(
                    "new mapping must use a fresh execution identity".into(),
                ));
            }
            let playback = if let Some(old) = self.arrays.get(id) {
                old.remapped_playback(definition.generation.clone())
            } else {
                Playback {
                    schema: 1,
                    history: compiled.configuration().history.clone(),
                    authority: compiled.configuration().authority.clone(),
                    revision: Revision(0),
                    generation: definition.generation.clone(),
                    level: Level::OFF,
                    effect: None,
                    parameters: BTreeMap::new(),
                    off_effect: None,
                    fade_ms: 0,
                }
            };
            arrays.insert(
                id.clone(),
                OrderedArray::new(
                    compiled.ownership()[id].clone(),
                    playback,
                    Effect::Delay { ticks: 0 },
                    execution.clone(),
                )?,
            );
        }
        if executions.keys().cloned().collect::<BTreeSet<_>>() != needed_executions
            || arrays
                .values()
                .map(|a| a.receipt().execution)
                .collect::<BTreeSet<_>>()
                .len()
                != arrays.len()
        {
            return Err(Error::Invalid(
                "configuration execution identities do not match changed mappings".into(),
            ));
        }
        let old_channels: BTreeSet<_> = self
            .configuration
            .ownership()
            .values()
            .flatten()
            .cloned()
            .collect();
        let new_channels: BTreeSet<_> = compiled.ownership().values().flatten().cloned().collect();
        let released = old_channels.difference(&new_channels).cloned().collect();
        let mut candidate = Self {
            configuration: compiled,
            arrays,
            revision: self.revision.next()?,
            document: Arc::from(""),
            retired_ids,
            edit_epoch: Arc::new(()),
        };
        candidate.document = Arc::from(
            candidate
                .capture(None, candidate.revision)?
                .encode_validated_for_commit()?,
        );
        check_deadline(deadline, clock())?;
        let revision = candidate.configuration().revision;
        *self = candidate;
        Ok(ConfigurationCommit { revision, released })
    }
    pub fn receipt(&self, id: &Id) -> Option<Receipt> {
        self.arrays.get(id).map(OrderedArray::receipt)
    }
    /// Immutable admission views use the same ordering guards as the owner,
    /// before BridgeKit can satisfy a request from its cached state.
    pub fn validate_playback(
        &self,
        id: &Id,
        request: &Playback,
        deadline: Option<Instant>,
        now: Instant,
    ) -> Result<(), Error> {
        self.array(id)?
            .validate_desired(request, deadline, now)
            .map(|_| ())
    }
    pub fn validate_control(
        &self,
        id: &Id,
        request: &Command,
        deadline: Option<Instant>,
        now: Instant,
    ) -> Result<(), Error> {
        self.array(id)?
            .validate_command(request, deadline, now)
            .map(|_| ())
    }
    /// Logical engine output, reported as assumed rather than fixture readback.
    pub fn states(&self) -> Result<BTreeMap<Id, ArrayState>, Error> {
        self.arrays
            .iter()
            .map(|(id, array)| {
                let receipt = array.receipt();
                Ok((
                    id.clone(),
                    ArrayState {
                        playback: array.playback().encode()?,
                        status: receipt.status,
                        execution: receipt.execution,
                        operation_revision: receipt.revision,
                    },
                ))
            })
            .collect()
    }
    pub fn output(&self, id: &Id, channel: &Channel) -> u8 {
        self.arrays.get(id).map_or(0, |a| a.output(channel))
    }
    pub fn desired(
        &mut self,
        id: &Id,
        request: Playback,
        execution: Id,
        deadline: Option<Instant>,
        clock: impl Fn() -> Instant,
    ) -> Result<Receipt, Error> {
        let admission_time = clock();
        if let Some(receipt) =
            self.array(id)?
                .validate_desired(&request, deadline, admission_time)?
        {
            check_deadline(deadline, clock())?;
            return Ok(receipt);
        }
        let previous = self.array(id)?.playback();
        for (selected, old) in [
            (&request.effect, &previous.effect),
            (&request.off_effect, &previous.off_effect),
        ] {
            if selected != old
                && selected
                    .as_ref()
                    .is_some_and(|effect| !self.configuration.has_effect(id, effect))
            {
                return Err(Error::Invalid(
                    "changed effect selection is not published for this array".into(),
                ));
            }
        }
        let effect = if self.array(id)?.starts_execution(&request) {
            self.configuration.effect(
                id,
                if request.level.is_on() {
                    request.effect.as_ref()
                } else {
                    request.off_effect.as_ref()
                },
                &request.parameters,
                !request.level.is_on(),
                request.fade_ms,
            )?
        } else {
            self.array(id)?.accepted_effect()
        };
        let mut candidate = self.array(id)?.clone();
        let receipt = candidate.desired(request, effect, execution, deadline, admission_time)?;
        self.commit(id, candidate, receipt, deadline, clock)
    }
    pub fn command(
        &mut self,
        id: &Id,
        request: Command,
        execution: Id,
        deadline: Option<Instant>,
        clock: impl Fn() -> Instant,
    ) -> Result<Receipt, Error> {
        let admission_time = clock();
        if let Some(receipt) =
            self.array(id)?
                .validate_command(&request, deadline, admission_time)?
        {
            check_deadline(deadline, clock())?;
            return Ok(receipt);
        }
        let effect = if request.restart {
            let playback = self.array(id)?.playback();
            Some(self.configuration.effect(
                id,
                if playback.level.is_on() {
                    playback.effect.as_ref()
                } else {
                    playback.off_effect.as_ref()
                },
                &playback.parameters,
                !playback.level.is_on(),
                playback.fade_ms,
            )?)
        } else {
            None
        };
        let mut candidate = self.array(id)?.clone();
        let receipt =
            candidate.command_with_effect(request, execution, effect, deadline, admission_time)?;
        self.commit(id, candidate, receipt, deadline, clock)
    }
    pub fn tick(&mut self) -> Result<bool, Error> {
        let mut completed = false;
        for array in self.arrays.values_mut() {
            let before = array.receipt().status;
            array.tick();
            completed |= before != array.receipt().status;
        }
        if completed {
            let revision = self.revision.next()?;
            let document = self
                .capture(None, revision)?
                .encode_validated_for_commit()?;
            self.document = Arc::from(document);
            self.revision = revision;
        }
        Ok(completed)
    }
    fn array(&self, id: &Id) -> Result<&OrderedArray, Error> {
        self.arrays
            .get(id)
            .ok_or_else(|| Error::Invalid("unknown array".into()))
    }
    fn capture(
        &self,
        candidate: Option<(&Id, &OrderedArray)>,
        revision: Revision,
    ) -> Result<Snapshot, Error> {
        let mut arrays = BTreeMap::new();
        for (id, array) in &self.arrays {
            let array = candidate
                .filter(|(target, _)| *target == id)
                .map_or(array, |(_, array)| array);
            arrays.insert(id.clone(), array.checkpoint()?);
        }
        Ok(Snapshot {
            schema: 1,
            history: self.configuration.configuration().history.clone(),
            authority: self.configuration.configuration().authority.clone(),
            revision,
            configuration: self.configuration.configuration().clone(),
            arrays,
            retired_ids: self.retired_ids.clone(),
        })
    }
    fn commit(
        &mut self,
        id: &Id,
        candidate: OrderedArray,
        receipt: Receipt,
        deadline: Option<Instant>,
        clock: impl Fn() -> Instant,
    ) -> Result<Receipt, Error> {
        if receipt.revision == self.array(id)?.receipt().revision {
            check_deadline(deadline, clock())?;
            return Ok(receipt);
        }
        let revision = self.revision.next()?;
        let document = self
            .capture(Some((id, &candidate)), revision)?
            .encode_validated_for_commit()?;
        check_deadline(deadline, clock())?;
        self.arrays.insert(id.clone(), candidate);
        self.edit_epoch = Arc::new(());
        self.revision = revision;
        self.document = Arc::from(document);
        Ok(receipt)
    }
}

fn check_deadline(deadline: Option<Instant>, now: Instant) -> Result<(), Error> {
    if deadline.is_some_and(|deadline| now >= deadline) {
        return Err(Error::Invalid(
            "request expired after preparation, before installation commit".into(),
        ));
    }
    Ok(())
}

fn object_ids(configuration: &Configuration) -> BTreeSet<Id> {
    let content = &configuration.content;
    content
        .universes
        .keys()
        .chain(content.arrays.keys())
        .chain(content.effects.keys())
        .chain(content.arrays.values().flat_map(|a| a.effects.keys()))
        .chain(content.arrays.values().map(|a| &a.generation))
        .cloned()
        .collect()
}

fn identity_kinds(configuration: &Configuration) -> BTreeMap<Id, u8> {
    let content = &configuration.content;
    content
        .universes
        .keys()
        .map(|id| (id.clone(), 0))
        .chain(content.arrays.keys().map(|id| (id.clone(), 1)))
        .chain(
            content
                .effects
                .keys()
                .chain(content.arrays.values().flat_map(|a| a.effects.keys()))
                .map(|id| (id.clone(), 2)),
        )
        .chain(content.arrays.values().map(|a| (a.generation.clone(), 3)))
        .collect()
}
