//! Serialized array admission. The service calls these methods at a tick boundary
//! with the original ingress deadline and an already resolved candidate effect.
use crate::engine::{Channel, Effect, Player, Status};
use dmx_protocol::{
    canonicalize_json, classify_operation, sha256, Admission, Error, Id, OperationStamp, Playback,
    Revision,
};
use serde::{Deserialize, Serialize};
use std::time::Instant;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub revision: Revision,
    pub execution: Id,
    pub status: Status,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    pub revision: Revision,
    pub expected_playback: Revision,
    pub expected_execution: Id,
    pub history: Id,
    pub authority: Id,
    pub generation: Id,
    pub restart: bool,
}

#[derive(Clone)]
pub struct OrderedArray {
    owned: Vec<Channel>,
    player: Player,
    playback: Playback,
    execution: Id,
    high_water: OperationStamp,
    last_receipt: Receipt,
    last_operation: Operation,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "request",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Operation {
    Desired(Playback),
    Command(Command),
}

/// Internal engine checkpoint. The installation recovery codec owns its wire
/// envelope and mapping; this record never trusts a separately supplied hash.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    pub playback: String,
    pub execution: Id,
    pub last_operation: Operation,
    pub receipt: Receipt,
    pub player: crate::engine::Snapshot,
}

impl OrderedArray {
    pub fn new(
        owned: Vec<Channel>,
        playback: Playback,
        effect: Effect,
        execution: Id,
    ) -> Result<Self, Error> {
        playback.encode()?;
        let player = Player::start(owned.clone(), effect, vec![], playback.level)?;
        let high_water = OperationStamp {
            revision: playback.revision,
            fingerprint: playback.fingerprint()?,
        };
        let last_receipt = Receipt {
            revision: playback.revision,
            execution: execution.clone(),
            status: player.status(),
        };
        let last_operation = Operation::Desired(playback.clone());
        Ok(Self {
            owned,
            player,
            playback,
            execution,
            high_water,
            last_receipt,
            last_operation,
        })
    }
    pub fn tick(&mut self) {
        self.player.tick();
    }
    pub fn output(&self, channel: &Channel) -> u8 {
        self.player.output(channel)
    }
    pub(crate) fn completed_off(&self) -> bool {
        !self.playback.level.is_on() && self.player.status() == Status::Completed
    }
    pub(crate) fn playback(&self) -> &Playback {
        &self.playback
    }
    pub(crate) fn remapped_playback(&self, generation: Id) -> Playback {
        let mut playback = self.playback.clone();
        playback.revision = self.high_water.revision;
        playback.generation = generation;
        playback
    }
    pub fn receipt(&self) -> Receipt {
        Receipt {
            revision: self.high_water.revision,
            execution: self.execution.clone(),
            status: self.player.status(),
        }
    }
    pub fn desired(
        &mut self,
        request: Playback,
        effect: Effect,
        execution: Id,
        deadline: Option<Instant>,
        now: Instant,
    ) -> Result<Receipt, Error> {
        if let Some(receipt) = self.validate_desired(&request, deadline, now)? {
            return Ok(receipt);
        }
        let stamp = OperationStamp {
            revision: request.revision,
            fingerprint: request.fingerprint()?,
        };
        // Construct before mutation, even when equal settings preserve the live
        // player: bad compiled candidates must never advance the high-water mark.
        let candidate = Player::start(
            self.owned.clone(),
            effect,
            self.player.snapshot().held,
            request.level,
        )?;
        let changed_selection = request.effect != self.playback.effect
            || request.parameters != self.playback.parameters;
        let restart = (request.level.is_on()
            && (!self.playback.level.is_on() || changed_selection))
            || (!request.level.is_on() && self.playback.level.is_on());
        if restart {
            self.check_execution(&execution)?;
            self.player = candidate;
            self.execution = execution;
        } else {
            self.player.set_level(request.level);
        }
        self.last_operation = Operation::Desired(request.clone());
        self.playback = request;
        Ok(self.commit(stamp))
    }
    /// Shared by the runtime admission view and the serialized owner. A cached
    /// receipt is usable only after identity, expiry and operation ordering pass.
    pub fn validate_desired(
        &self,
        request: &Playback,
        deadline: Option<Instant>,
        now: Instant,
    ) -> Result<Option<Receipt>, Error> {
        self.guard(
            &request.history,
            &request.authority,
            &request.generation,
            deadline,
            now,
        )?;
        let stamp = OperationStamp {
            revision: request.revision,
            fingerprint: request.fingerprint()?,
        };
        if classify_operation(Some(&self.high_water), &stamp)? == Admission::Duplicate {
            return Ok(Some(self.last_receipt.clone()));
        }
        Ok(None)
    }
    pub fn command(
        &mut self,
        request: Command,
        execution: Id,
        deadline: Option<Instant>,
        now: Instant,
    ) -> Result<Receipt, Error> {
        self.command_with_effect(request, execution, None, deadline, now)
    }
    pub(crate) fn command_with_effect(
        &mut self,
        request: Command,
        execution: Id,
        effect: Option<Effect>,
        deadline: Option<Instant>,
        now: Instant,
    ) -> Result<Receipt, Error> {
        if let Some(receipt) = self.validate_command(&request, deadline, now)? {
            return Ok(receipt);
        }
        let canonical = canonicalize_json(&serde_json::to_vec(&request)?, 16 * 1024)?;
        let stamp = OperationStamp {
            revision: request.revision,
            fingerprint: sha256(canonical.as_bytes()),
        };
        if request.restart {
            self.check_execution(&execution)?;
            let snapshot = self.player.snapshot();
            let restarted = Player::start(
                self.owned.clone(),
                effect.unwrap_or(snapshot.effect),
                snapshot.initial,
                self.playback.level,
            )?;
            self.player = restarted;
            self.execution = execution;
        } else {
            self.player.stop();
        }
        self.last_operation = Operation::Command(request);
        Ok(self.commit(stamp))
    }
    pub fn validate_command(
        &self,
        request: &Command,
        deadline: Option<Instant>,
        now: Instant,
    ) -> Result<Option<Receipt>, Error> {
        self.guard(
            &request.history,
            &request.authority,
            &request.generation,
            deadline,
            now,
        )?;
        let canonical = canonicalize_json(&serde_json::to_vec(request)?, 16 * 1024)?;
        let stamp = OperationStamp {
            revision: request.revision,
            fingerprint: sha256(canonical.as_bytes()),
        };
        if classify_operation(Some(&self.high_water), &stamp)? == Admission::Duplicate {
            return Ok(Some(self.last_receipt.clone()));
        }
        if request.expected_playback != self.playback.revision
            || request.expected_execution != self.execution
        {
            return Err(Error::Invalid(
                "command playback/execution precondition failed".into(),
            ));
        }
        Ok(None)
    }
    pub fn checkpoint(&self) -> Result<Checkpoint, Error> {
        let mut player = self.player.snapshot();
        if player.status == Status::Running {
            // Running recovery restarts from initial planes. Keeping frame output
            // here would change the document without a transition/revision.
            player.held = player.initial.clone();
        }
        Ok(Checkpoint {
            playback: self.playback.encode()?,
            execution: self.execution.clone(),
            last_operation: self.last_operation.clone(),
            receipt: self.last_receipt.clone(),
            player,
        })
    }
    pub fn restore(owned: Vec<Channel>, checkpoint: Checkpoint) -> Result<Self, Error> {
        let playback = Playback::decode(checkpoint.playback.as_bytes())?;
        let invalid = || Error::Invalid("inconsistent array recovery record".into());
        if checkpoint.playback != playback.encode()?
            || checkpoint.execution != checkpoint.receipt.execution
            || checkpoint.player.level != playback.level
            || !(checkpoint.player.status == checkpoint.receipt.status
                || (checkpoint.receipt.status == Status::Running
                    && checkpoint.player.status == Status::Completed))
        {
            return Err(invalid());
        }
        let high_water = match &checkpoint.last_operation {
            Operation::Desired(request) => {
                if request != &playback || request.revision != checkpoint.receipt.revision {
                    return Err(invalid());
                }
                OperationStamp {
                    revision: request.revision,
                    fingerprint: request.fingerprint()?,
                }
            }
            Operation::Command(command) => {
                if command.revision != checkpoint.receipt.revision
                    || command.revision <= playback.revision
                    || command.expected_playback != playback.revision
                    || command.history != playback.history
                    || command.authority != playback.authority
                    || command.generation != playback.generation
                    || (command.restart
                        && (command.expected_execution == checkpoint.execution
                            || checkpoint.receipt.status == Status::Stopped))
                    || (!command.restart
                        && (command.expected_execution != checkpoint.execution
                            || checkpoint.receipt.status != Status::Stopped))
                {
                    return Err(invalid());
                }
                let canonical = canonicalize_json(&serde_json::to_vec(command)?, 16 * 1024)?;
                OperationStamp {
                    revision: command.revision,
                    fingerprint: sha256(canonical.as_bytes()),
                }
            }
        };
        let status = checkpoint.player.status;
        let expected_player = checkpoint.player.clone();
        let player = Player::restore(owned.clone(), checkpoint.player)?;
        if player.status() != status || player.snapshot() != expected_player {
            return Err(invalid());
        }
        Ok(Self {
            owned,
            player,
            playback,
            execution: checkpoint.execution,
            high_water,
            last_receipt: checkpoint.receipt,
            last_operation: checkpoint.last_operation,
        })
    }
    fn commit(&mut self, stamp: OperationStamp) -> Receipt {
        self.high_water = stamp;
        self.last_receipt = self.receipt();
        self.last_receipt.clone()
    }
    fn check_execution(&self, execution: &Id) -> Result<(), Error> {
        if execution == &self.execution {
            return Err(Error::Invalid(
                "new execution requires a fresh identity".into(),
            ));
        }
        Ok(())
    }
    fn guard(
        &self,
        history: &Id,
        authority: &Id,
        generation: &Id,
        deadline: Option<Instant>,
        now: Instant,
    ) -> Result<(), Error> {
        if deadline.is_some_and(|deadline| now >= deadline) {
            return Err(Error::Invalid(
                "request expired before engine commit".into(),
            ));
        }
        if history != &self.playback.history
            || authority != &self.playback.authority
            || generation != &self.playback.generation
        {
            return Err(Error::Invalid(
                "request history/authority/generation is fenced".into(),
            ));
        }
        Ok(())
    }
}
