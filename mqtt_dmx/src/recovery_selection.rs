//! Startup/reconnect selection over validated snapshot cuts. This component never
//! restores a player or changes output; the owner prepares and installs the decision.
use crate::installation::Installation;
use dmx_protocol::{Id, Revision};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Waiting,
    ProvisionalFallback,
    Authoritative,
    RecoveryFault,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cut {
    history: Id,
    authority: Id,
    revision: Revision,
    digest: String,
}
impl Cut {
    pub fn of(installation: &Installation) -> Self {
        Self {
            history: installation.configuration().history.clone(),
            authority: installation.configuration().authority.clone(),
            revision: installation.revision(),
            digest: dmx_protocol::sha256(installation.document().as_bytes()),
        }
    }
    fn same_identity(&self, other: &Self) -> bool {
        self.history == other.history && self.authority == other.authority
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fault {
    InvalidSnapshot(String),
    ForeignIdentity,
    ConflictingRevision,
    UnexpectedNewerSnapshot,
    MissingLocalState,
    InconsistentFallbackBase,
    ExplicitReconciliationRequired,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    AdoptBroker,
    KeepLive,
    PromoteLive,
    WaitForInitialization,
    Fault(Fault),
}

pub fn decide(
    mode: Mode,
    live: Option<&Cut>,
    saved_base: Option<&Cut>,
    broker: Result<Option<&Cut>, &str>,
) -> Decision {
    use std::cmp::Ordering;
    let broker = match broker {
        Ok(broker) => broker,
        Err(error) => return Decision::Fault(Fault::InvalidSnapshot(error.to_owned())),
    };
    match mode {
        Mode::RecoveryFault => Decision::Fault(Fault::ExplicitReconciliationRequired),
        Mode::Waiting => {
            if live.is_some() {
                return Decision::Fault(Fault::MissingLocalState);
            }
            if broker.is_some() {
                Decision::AdoptBroker
            } else {
                Decision::WaitForInitialization
            }
        }
        Mode::ProvisionalFallback => {
            let (Some(live), Some(base)) = (live, saved_base) else {
                return Decision::Fault(Fault::MissingLocalState);
            };
            if !live.same_identity(base)
                || live.revision < base.revision
                || (live.revision == base.revision && live.digest != base.digest)
            {
                return Decision::Fault(Fault::InconsistentFallbackBase);
            }
            let Some(broker) = broker else {
                return Decision::PromoteLive;
            };
            if !broker.same_identity(base) {
                return Decision::Fault(Fault::ForeignIdentity);
            }
            match broker.revision.cmp(&base.revision) {
                Ordering::Greater => Decision::AdoptBroker,
                Ordering::Less => Decision::PromoteLive,
                Ordering::Equal if broker.digest == base.digest => Decision::PromoteLive,
                Ordering::Equal => Decision::Fault(Fault::ConflictingRevision),
            }
        }
        Mode::Authoritative => {
            let Some(live) = live else {
                return Decision::Fault(Fault::MissingLocalState);
            };
            let Some(broker) = broker else {
                return Decision::KeepLive;
            };
            if !broker.same_identity(live) {
                return Decision::Fault(Fault::ForeignIdentity);
            }
            match broker.revision.cmp(&live.revision) {
                Ordering::Less => Decision::KeepLive,
                Ordering::Equal if broker.digest == live.digest => Decision::KeepLive,
                Ordering::Equal => Decision::Fault(Fault::ConflictingRevision),
                Ordering::Greater => Decision::Fault(Fault::UnexpectedNewerSnapshot),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dmx_protocol::configuration::{Configuration, Content};
    use std::{collections::BTreeMap, time::Instant};
    fn id(n: u8) -> Id {
        format!("00000000-0000-0000-0000-{n:012}")
            .try_into()
            .unwrap()
    }
    // Cut values come from the real canonical installation writer, not fabricated digests.
    fn cut(history: u8, setting: &str, revision: u64) -> Cut {
        let mut content = Content::default();
        content.values.insert("setting".into(), setting.into());
        let configuration = Configuration {
            schema: 1,
            history: id(history),
            authority: id(history + 1),
            revision: Revision(1),
            expected_previous: None,
            digest: content.digest().unwrap(),
            content,
        };
        let mut installation = Installation::initialize(configuration, BTreeMap::new()).unwrap();
        for revision in 2..=revision {
            let mut next = installation.configuration().clone();
            next.revision = Revision(revision);
            next.expected_previous = Some(Revision(revision - 1));
            next.content
                .values
                .insert("cut_tick".into(), revision.to_string());
            next.digest = next.content.digest().unwrap();
            installation
                .configure(next, BTreeMap::new(), None, Instant::now)
                .unwrap();
        }
        Cut::of(&installation)
    }

    #[test]
    fn waiting_needs_proven_absence_or_a_valid_broker_cut() {
        let broker = cut(10, "broker", 1);
        assert_eq!(
            decide(Mode::Waiting, None, None, Ok(None)),
            Decision::WaitForInitialization
        );
        assert_eq!(
            decide(Mode::Waiting, None, None, Ok(Some(&broker))),
            Decision::AdoptBroker
        );
        assert_eq!(
            decide(Mode::Waiting, None, None, Err("corrupt")),
            Decision::Fault(Fault::InvalidSnapshot("corrupt".into()))
        );
    }
    #[test]
    fn provisional_reconnect_compares_against_the_saved_base_not_later_local_completion() {
        let older = cut(10, "base", 1);
        let base = cut(10, "base", 2);
        let live = cut(10, "base", 4);
        let newer = cut(10, "newer", 3);
        for broker in [None, Some(&older), Some(&base)] {
            assert_eq!(
                decide(
                    Mode::ProvisionalFallback,
                    Some(&live),
                    Some(&base),
                    Ok(broker)
                ),
                Decision::PromoteLive,
                "matching or older replay restarted provisional playback"
            );
        }
        assert_eq!(
            decide(
                Mode::ProvisionalFallback,
                Some(&live),
                Some(&base),
                Ok(Some(&newer))
            ),
            Decision::AdoptBroker,
            "newer broker cut was compared to local completions instead of saved base"
        );
    }
    #[test]
    fn conflicting_provisional_history_or_equal_revision_never_changes_output_automatically() {
        let base = cut(10, "base", 2);
        let live = cut(10, "base", 3);
        let conflicting = cut(10, "different", 2);
        let foreign = cut(20, "base", 1);
        assert_eq!(
            decide(
                Mode::ProvisionalFallback,
                Some(&live),
                Some(&base),
                Ok(Some(&conflicting))
            ),
            Decision::Fault(Fault::ConflictingRevision)
        );
        assert_eq!(
            decide(
                Mode::ProvisionalFallback,
                Some(&live),
                Some(&base),
                Ok(Some(&foreign))
            ),
            Decision::Fault(Fault::ForeignIdentity)
        );
    }
    #[test]
    fn authoritative_reconnect_keeps_live_state_without_restart_or_rollback() {
        let live = cut(10, "live", 3);
        let older = cut(10, "old", 2);
        for broker in [None, Some(&older), Some(&live)] {
            assert_eq!(
                decide(Mode::Authoritative, Some(&live), None, Ok(broker)),
                Decision::KeepLive
            );
        }
    }
    #[test]
    fn authoritative_reconnect_refuses_newer_foreign_and_equal_conflicting_snapshots() {
        let live = cut(10, "live", 3);
        let newer = cut(10, "live", 4);
        let foreign = cut(20, "live", 2);
        let conflicting = cut(10, "changed", 3);
        assert_eq!(
            decide(Mode::Authoritative, Some(&live), None, Ok(Some(&newer))),
            Decision::Fault(Fault::UnexpectedNewerSnapshot)
        );
        assert_eq!(
            decide(Mode::Authoritative, Some(&live), None, Ok(Some(&foreign))),
            Decision::Fault(Fault::ForeignIdentity)
        );
        assert_eq!(
            decide(
                Mode::Authoritative,
                Some(&live),
                None,
                Ok(Some(&conflicting))
            ),
            Decision::Fault(Fault::ConflictingRevision)
        );
    }
    #[test]
    fn faulted_or_inconsistent_local_state_requires_explicit_reconciliation() {
        let base = cut(10, "base", 2);
        let older_live = cut(10, "base", 1);
        let foreign_live = cut(20, "base", 3);
        assert_eq!(
            decide(Mode::RecoveryFault, Some(&base), None, Ok(Some(&base))),
            Decision::Fault(Fault::ExplicitReconciliationRequired)
        );
        assert_eq!(
            decide(Mode::Authoritative, None, None, Ok(Some(&base))),
            Decision::Fault(Fault::MissingLocalState)
        );
        for live in [&older_live, &foreign_live] {
            assert_eq!(
                decide(Mode::ProvisionalFallback, Some(live), Some(&base), Ok(None)),
                Decision::Fault(Fault::InconsistentFallbackBase)
            );
        }
    }
}
