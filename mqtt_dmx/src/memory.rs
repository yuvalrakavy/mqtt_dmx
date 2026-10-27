//! Conservative engine/recovery admission units, not allocator/RSS measurement.
//! Each decoded/compiled preparation fits one slot. Six slots cover the live
//! installation, candidate, captured and completion-reserved checkpoints, JSON
//! expansion and codec validation replacement (including compiler scratch players).
//! Five more cover the one concurrent fallback decoder/validator and its scratch
//! player/plan. Sixteen bounded wire
//! buffers reserve the five recovery roles plus engine/fallback codec scratch.
use crate::{engine::Effect, recovery::Snapshot};
use dmx_protocol::{Error, MAX_RECOVERY_BYTES};
use serde_json::Value;
mod meter;

pub const MAX_ENGINE_RECOVERY_BYTES: usize = 16 * 1024 * 1024;
pub const ENGINE_SLOTS: usize = 11;
pub const RECOVERY_BUFFERS: usize = 16;
pub const RECOVERY_RESERVE: usize = RECOVERY_BUFFERS * MAX_RECOVERY_BYTES;
pub const ENGINE_SLOT_BYTES: usize = (MAX_ENGINE_RECOVERY_BYTES - RECOVERY_RESERVE) / ENGINE_SLOTS;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reservation {
    pub decoded: usize,
    /// Includes every fade's future initial-plane map, even before its first tick.
    pub runtime: usize,
    pub slot: usize,
    pub recovery: usize,
    pub total: usize,
}

fn sum(values: impl IntoIterator<Item = usize>) -> usize {
    values.into_iter().fold(0, usize::saturating_add)
}

fn decoded(value: &Value) -> usize {
    match value {
        // Per-member units reserve map nodes, typed fields, key storage and
        // serializer/decoder expansion; empty values still carry that cost.
        Value::Object(values) => 64usize.saturating_add(sum(values
            .iter()
            .map(|(key, value)| sum([64, key.capacity(), decoded(value)])))),
        Value::Array(values) => sum([
            64,
            values.capacity().saturating_mul(64),
            sum(values.iter().map(decoded)),
        ]),
        Value::String(value) => 32usize.saturating_add(value.capacity()),
        _ => 32,
    }
}

fn runtime(effect: &Effect) -> usize {
    // Compiled/runtime node storage plus targets and their lazily created
    // initial-plane maps. Reserve every branch, including future Sequence nodes.
    512usize.saturating_add(match effect {
        Effect::Fade { targets, .. } => targets.capacity().saturating_mul(512),
        Effect::Sequence { nodes } | Effect::Parallel { nodes } => sum([
            nodes.capacity().saturating_mul(128),
            sum(nodes.iter().map(runtime)),
        ]),
        Effect::Delay { .. } => 0,
    })
}

pub(crate) fn reserve_player(effect: &Effect, owned: usize) -> Result<(), Error> {
    let fixed = sum([runtime(effect), owned.saturating_mul(1024)]);
    if fixed > ENGINE_SLOT_BYTES {
        return Err(Error::Invalid(
            "player exceeds 16MiB aggregate engine/recovery admission slot".into(),
        ));
    }
    let plan = meter::measure(effect, ENGINE_SLOT_BYTES - fixed)?;
    let slot = sum([plan, fixed]);
    if slot > ENGINE_SLOT_BYTES {
        return Err(Error::Invalid(
            "player exceeds 16MiB aggregate engine/recovery admission slot".into(),
        ));
    }
    Ok(())
}

pub fn reserve(snapshot: &Snapshot) -> Result<Reservation, Error> {
    let mut decoded_bytes = meter::measure(snapshot, ENGINE_SLOT_BYTES)?;
    // Authored maps survive as typed BTreeMaps in addition to their decoded
    // representation. Plain serialized struct fields do not need this charge.
    let content = &snapshot.configuration.content;
    let map_members = sum([
        content.universes.len(),
        content.arrays.len(),
        content.effects.len(),
        content.values.len(),
        sum(content.arrays.values().map(|array| {
            sum([
                array.lights.len(),
                array.defaults.len(),
                array.effects.len(),
            ])
        })),
    ]);
    decoded_bytes = decoded_bytes.saturating_add(map_members.saturating_mul(128));
    // Playback is an encoded string in a checkpoint, but a decoded document in
    // the live OrderedArray; count its expansion as well as the carrier string.
    for array in snapshot.arrays.values() {
        let playback: Value = dmx_protocol::decode_strict(array.playback.as_bytes(), 16 * 1024)?;
        decoded_bytes = decoded_bytes.saturating_add(decoded(&playback));
    }
    let runtime_bytes = sum(snapshot
        .arrays
        .values()
        .map(|array| runtime(&array.player.effect)));
    let slot = decoded_bytes.saturating_add(runtime_bytes);
    let total = slot
        .saturating_mul(ENGINE_SLOTS)
        .saturating_add(RECOVERY_RESERVE);
    if slot > ENGINE_SLOT_BYTES || total > MAX_ENGINE_RECOVERY_BYTES {
        return Err(Error::Invalid(
            "installation exceeds 16MiB aggregate engine/recovery admission budget".into(),
        ));
    }
    Ok(Reservation {
        decoded: decoded_bytes,
        runtime: runtime_bytes,
        slot,
        recovery: RECOVERY_RESERVE,
        total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Channel, Target};

    #[test]
    fn large_ownership_is_charged_even_for_a_delay() {
        use crate::engine::Player;
        let channels: Vec<_> = (0..1200)
            .map(|n| Channel {
                endpoint: "127.0.0.1:6454".parse().unwrap(),
                net: 0,
                subnet: 0,
                universe: (n / 512) as u8,
                channel: (n % 512) as u16,
            })
            .collect();
        assert!(
            matches!(Player::start(channels, Effect::Delay { ticks: 1 }, vec![], dmx_protocol::Level::FULL),
            Err(ref error) if error.to_string().contains("16MiB")),
            "large ownership bypassed player admission for a small plan"
        );
    }

    #[test]
    fn sparse_authored_node_capacity_is_reserved() {
        use crate::engine::Player;
        let mut nodes = Vec::with_capacity(10_000);
        nodes.push(Effect::Delay { ticks: 1 });
        assert!(
            matches!(Player::start(Vec::<Channel>::new(), Effect::Sequence { nodes }, vec![], dmx_protocol::Level::FULL),
            Err(ref error) if error.to_string().contains("16MiB")),
            "sparse authored node capacity bypassed player admission"
        );
    }

    #[test]
    fn reservations_cover_simultaneous_slots_recovery_and_future_fade_maps() {
        assert_eq!(
            ENGINE_SLOTS, 11,
            "concurrent fallback slots were not reserved"
        );
        assert_eq!(
            RECOVERY_RESERVE,
            4 * 1024 * 1024,
            "recovery wire copies were not reserved"
        );
        assert_eq!(ENGINE_SLOT_BYTES, 1_143_901, "aggregate partition changed");
        let target = Target {
            channel: Channel {
                endpoint: "127.0.0.1:6454".parse().unwrap(),
                net: 0,
                subnet: 0,
                universe: 0,
                channel: 0,
            },
            planes: dmx_protocol::ChannelPlanes {
                dimmable: 255,
                independent: 0,
            },
        };
        let nested = Effect::Sequence {
            nodes: vec![
                Effect::Delay { ticks: 50 },
                Effect::Fade {
                    ticks: 100,
                    targets: vec![target],
                },
            ],
        };
        assert!(
            runtime(&nested) >= 4 * 512,
            "a future fade's initial map was not reserved"
        );
        assert!(
            decoded(&serde_json::json!({"empty":""})) >= 160,
            "empty JSON fields lost their allocation cost"
        );
        assert_eq!(
            sum([usize::MAX, 1]),
            usize::MAX,
            "memory accounting wrapped"
        );
    }

    #[test]
    fn expanded_player_is_refused_before_runtime_allocation_and_small_player_still_runs() {
        use crate::engine::{Player, Target};
        let channels: Vec<_> = (0..32)
            .map(|channel| Channel {
                endpoint: "127.0.0.1:6454".parse().unwrap(),
                net: 0,
                subnet: 0,
                universe: 0,
                channel,
            })
            .collect();
        let targets: Vec<_> = channels
            .iter()
            .cloned()
            .map(|channel| Target {
                channel,
                planes: dmx_protocol::ChannelPlanes {
                    dimmable: 255,
                    independent: 0,
                },
            })
            .collect();
        let effect = Effect::Parallel {
            nodes: (0..512)
                .map(|_| Effect::Fade {
                    ticks: 100,
                    targets: targets.clone(),
                })
                .collect(),
        };
        assert!(
            matches!(Player::start(channels.clone(), effect, vec![], dmx_protocol::Level::FULL), Err(ref error) if error.to_string().contains("16MiB")),
            "expanded execution bypassed its aggregate memory slot"
        );
        let mut player = Player::start(
            channels,
            Effect::Delay { ticks: 1 },
            vec![],
            dmx_protocol::Level::FULL,
        )
        .unwrap();
        player.tick();
        assert_eq!(player.status(), crate::engine::Status::Completed);
    }
}
