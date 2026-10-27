//! One candidate prepared off the lighting loop, then rebased and swapped by
//! its owner. This API owns no threads, queues, clocks or I/O.
use super::*;

pub enum Edit {
    Configuration {
        configuration: Configuration,
        executions: BTreeMap<Id, Id>,
    },
    Desired {
        array: Id,
        playback: Playback,
        execution: Id,
    },
    Command {
        array: Id,
        command: Command,
        execution: Id,
    },
}

pub enum EditOutcome {
    Configuration(ConfigurationCommit),
    Playback { array: Id, receipt: Receipt },
}

pub struct Preparation {
    candidate: Installation,
    base: Arc<()>,
    edit: Edit,
    deadline: Option<Instant>,
}

pub struct PreparedEdit {
    candidate: Installation,
    base: Arc<()>,
    outcome: EditOutcome,
    deadline: Option<Instant>,
}

impl Installation {
    /// Capture one bounded candidate for the service's single preparation worker.
    /// Only ticks may intervene before commit; an accepted edit/replacement
    /// invalidates this candidate even if its persisted identities match.
    pub fn preparation(&self, edit: Edit, deadline: Option<Instant>) -> Preparation {
        Preparation {
            candidate: self.clone(),
            base: self.edit_epoch.clone(),
            edit,
            deadline,
        }
    }
}

impl Preparation {
    /// Run compilation/validation on the worker, never on the lighting loop.
    pub fn run(mut self) -> Result<PreparedEdit, Error> {
        let outcome = match self.edit {
            Edit::Configuration {
                configuration,
                executions,
            } => EditOutcome::Configuration(self.candidate.configure(
                configuration,
                executions,
                self.deadline,
                Instant::now,
            )?),
            Edit::Desired {
                array,
                playback,
                execution,
            } => {
                let receipt = self.candidate.desired(
                    &array,
                    playback,
                    execution,
                    self.deadline,
                    Instant::now,
                )?;
                EditOutcome::Playback { array, receipt }
            }
            Edit::Command {
                array,
                command,
                execution,
            } => {
                let receipt = self.candidate.command(
                    &array,
                    command,
                    execution,
                    self.deadline,
                    Instant::now,
                )?;
                EditOutcome::Playback { array, receipt }
            }
        };
        Ok(PreparedEdit {
            candidate: self.candidate,
            base: self.base,
            outcome,
            deadline: self.deadline,
        })
    }
}

impl PreparedEdit {
    /// The owner calls this at a frame boundary. A refused candidate never
    /// changes the live installation, even after partial candidate rebasing.
    pub fn commit(
        mut self,
        live: &mut Installation,
        clock: impl Fn() -> Instant,
    ) -> Result<EditOutcome, Error> {
        check_deadline(self.deadline, clock())?;
        if !Arc::ptr_eq(&self.base, &live.edit_epoch) {
            return Err(Error::Invalid(
                "prepared edit belongs to a retired installation cut".into(),
            ));
        }
        if Arc::ptr_eq(&self.base, &self.candidate.edit_epoch) {
            // Duplicate request/configuration: keep the exact cached receipt,
            // latest completed state and document without publishing a new cut.
            return Ok(self.outcome);
        }
        let target = match &self.outcome {
            EditOutcome::Playback { array, .. } => Some(array),
            EditOutcome::Configuration(_) => None,
        };
        for (id, candidate) in &mut self.candidate.arrays {
            if let Some(previous) = live.arrays.get(id) {
                candidate.rebase_prepared_from(previous, target == Some(id))?;
            }
        }
        self.candidate.revision = live.revision.next()?;
        self.candidate.document = Arc::from(
            self.candidate
                .capture(None, self.candidate.revision)?
                .encode_validated_for_commit()?,
        );
        check_deadline(self.deadline, clock())?;
        if let EditOutcome::Playback { array, receipt } = &mut self.outcome {
            *receipt = self.candidate.array(array)?.receipt();
        }
        *live = self.candidate;
        Ok(self.outcome)
    }
}
