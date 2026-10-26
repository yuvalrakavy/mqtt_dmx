use dmx_protocol::{
    configuration::{Array, Configuration, Content, Universe},
    Id, Level, Playback, Revision,
};
use mqtt_dmx::{
    engine::{Channel, Status},
    installation::Installation,
    ordered_array::Command,
};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
fn id(n: u8) -> Id {
    format!("00000000-0000-0000-0000-{n:012}")
        .try_into()
        .unwrap()
}
fn config() -> Configuration {
    let mut content = Content::default();
    content.universes.insert(
        id(1),
        Universe {
            name: "main".into(),
            description: "".into(),
            endpoint: "127.0.0.1:6454".parse().unwrap(),
            net: 0,
            subnet: 0,
            universe: 0,
            channels: 512,
        },
    );
    content.arrays.insert(
        id(2),
        Array {
            name: "one".into(),
            description: "".into(),
            generation: id(3),
            universe: "main".into(),
            lights: BTreeMap::from([("all".into(), "1".into())]),
            on: None,
            off: None,
            effects: BTreeMap::new(),
            defaults: BTreeMap::new(),
        },
    );
    Configuration {
        schema: 1,
        history: id(4),
        authority: id(5),
        revision: Revision(1),
        expected_previous: None,
        digest: content.digest().unwrap(),
        content,
    }
}
fn installation() -> Installation {
    Installation::initialize(config(), BTreeMap::from([(id(2), id(6))])).unwrap()
}
fn request(revision: u64) -> Playback {
    Playback {
        schema: 1,
        history: id(4),
        authority: id(5),
        revision: Revision(revision),
        generation: id(3),
        level: Level::FULL,
        effect: None,
        parameters: BTreeMap::new(),
        off_effect: None,
        fade_ms: 200,
    }
}
fn channel() -> Channel {
    Channel {
        endpoint: "127.0.0.1:6454".parse().unwrap(),
        net: 0,
        subnet: 0,
        universe: 0,
        channel: 1,
    }
}

#[test]
fn probe_two_arrays_keep_independent_candidates_and_completion() {
    let mut c = config();
    let mut second = c.content.arrays[&id(2)].clone();
    second.name = "two".into();
    second.generation = id(9);
    second.lights = BTreeMap::from([("all".into(), "2".into())]);
    c.content.arrays.insert(id(8), second);
    c.digest = c.content.digest().unwrap();
    let mut i =
        Installation::initialize(c, BTreeMap::from([(id(2), id(6)), (id(8), id(10))])).unwrap();
    let now = Instant::now();
    let mut p = request(1);
    p.generation = id(9);
    i.desired(&id(8), p, id(11), None, || now)
        .expect("second array candidate must not replace first array checkpoint");
    i.tick().unwrap();
    let mut first = request(1);
    first.fade_ms = 50;
    i.desired(&id(2), first, id(7), None, || now).unwrap();
    assert_eq!(i.receipt(&id(8)).unwrap().execution, id(11));
    let mut second_channel = channel();
    second_channel.channel = 2;
    assert_eq!(i.output(&id(8), &second_channel), 64);
    i.tick().unwrap();
    assert_eq!(i.receipt(&id(2)).unwrap().status, Status::Completed);
    assert_eq!(i.receipt(&id(8)).unwrap().status, Status::Running);
    assert_eq!(i.output(&id(8), &second_channel), 128);
    let recovered = Installation::restore(i.document().as_bytes()).unwrap();
    assert_eq!(recovered.output(&id(2), &channel()), 255);
    assert_eq!(recovered.receipt(&id(8)).unwrap().execution, id(11));
    assert_eq!(recovered.receipt(&id(8)).unwrap().status, Status::Running);
}

#[test]
fn frames_do_not_publish_but_acceptance_and_completion_each_capture_once() {
    let mut i = installation();
    let now = Instant::now();
    assert_eq!(i.output(&id(2), &channel()), 0);
    assert_eq!(i.receipt(&id(2)).unwrap().status, Status::Completed);
    i.desired(&id(2), request(1), id(7), None, || now).unwrap();
    let start = i.document();
    let revision = i.revision();
    for _ in 0..3 {
        assert!(!i.tick().unwrap());
        assert_eq!(i.document(), start);
    }
    assert!(i.tick().unwrap());
    assert_eq!(i.revision(), revision.next().unwrap());
    assert_eq!(i.output(&id(2), &channel()), 255);
    let completed = i.document();
    for _ in 0..4 {
        assert!(!i.tick().unwrap());
    }
    assert_eq!(i.document(), completed);
}

#[test]
fn preparation_expiry_rejects_without_mutating_live_state_or_high_water() {
    let mut i = installation();
    let now = Instant::now();
    let before = i.document();
    let deadline = now + Duration::from_secs(1);
    let calls = std::cell::Cell::new(0);
    assert!(i
        .desired(&id(2), request(9), id(7), Some(deadline), || {
            let count = calls.get();
            calls.set(count + 1);
            if count == 0 {
                now
            } else {
                deadline
            }
        })
        .is_err());
    assert_eq!(i.document(), before);
    assert_eq!(i.output(&id(2), &channel()), 0);
    assert_eq!(
        i.desired(&id(2), request(1), id(8), None, || now)
            .unwrap()
            .revision,
        Revision(1)
    );
}

#[test]
fn brightness_commit_clones_runtime_position_instead_of_restarting_it() {
    let mut i = installation();
    let now = Instant::now();
    i.desired(&id(2), request(1), id(7), None, || now).unwrap();
    i.tick().unwrap();
    let mut dim = request(2);
    dim.level = Level::try_from(500).unwrap();
    assert_eq!(
        i.desired(&id(2), dim, id(8), None, || now)
            .unwrap()
            .execution,
        id(7)
    );
    i.tick().unwrap();
    assert_eq!(i.output(&id(2), &channel()), 64);
}

#[test]
fn stop_capture_survives_restart_and_stale_desired_cannot_undo_it() {
    let mut i = installation();
    let now = Instant::now();
    i.desired(&id(2), request(1), id(7), None, || now).unwrap();
    i.tick().unwrap();
    let command = Command {
        revision: Revision(4),
        expected_playback: Revision(1),
        expected_execution: id(7),
        history: id(4),
        authority: id(5),
        generation: id(3),
        restart: false,
    };
    let receipt = i
        .command(&id(2), command.clone(), id(8), None, || now)
        .unwrap();
    let saved = i.document();
    let mut restored = Installation::restore(saved.as_bytes()).unwrap();
    assert_eq!(restored.output(&id(2), &channel()), 64);
    assert!(restored
        .desired(&id(2), request(3), id(9), None, || now)
        .is_err());
    let revision = restored.revision();
    assert_eq!(
        restored
            .command(&id(2), command, id(9), None, || now)
            .unwrap(),
        receipt
    );
    assert_eq!(restored.revision(), revision);
    assert!(!restored.tick().unwrap());
    assert_eq!(restored.output(&id(2), &channel()), 64);
}

#[test]
fn captured_document_is_immutable_while_later_playback_completes() {
    let mut i = installation();
    let now = Instant::now();
    i.desired(&id(2), request(1), id(7), None, || now).unwrap();
    let captured = i.document();
    for _ in 0..4 {
        i.tick().unwrap();
    }
    assert_ne!(i.document(), captured);
    let restored = Installation::restore(captured.as_bytes()).unwrap();
    assert_eq!(restored.output(&id(2), &channel()), 0);
    assert_eq!(restored.receipt(&id(2)).unwrap().status, Status::Running);
}

#[test]
fn oversized_candidate_is_refused_before_output_or_operation_revision_changes() {
    let mut c = config();
    c.content.universes.get_mut(&id(1)).unwrap().description = "x".repeat(110_000);
    // A measured 320-channel candidate has a 255,531-byte carrier. Add 32
    // channels so the initial Off image fits but the running image exceeds 256KiB.
    c.content.arrays.get_mut(&id(2)).unwrap().lights.insert(
        "all".into(),
        (0..352)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(","),
    );
    c.digest = c.content.digest().unwrap();
    let mut i = Installation::initialize(c, BTreeMap::from([(id(2), id(6))])).unwrap();
    let before = i.document();
    let now = Instant::now();
    let result = i.desired(&id(2), request(1), id(7), None, || now);
    assert!(
        result.is_err(),
        "candidate still fits: raw {}, carrier {}",
        i.document().len(),
        serde_json::to_vec(&serde_json::json!({"snapshot":i.document().as_ref()}))
            .unwrap()
            .len()
    );
    assert_eq!(i.document(), before);
    assert_eq!(i.output(&id(2), &channel()), 0);
    assert_eq!(i.receipt(&id(2)).unwrap().revision, Revision(0));
}

#[test]
fn restored_revision_reserves_completion_headroom() {
    let mut i = installation();
    let now = Instant::now();
    i.desired(&id(2), request(1), id(7), None, || now).unwrap();
    let mut s: mqtt_dmx::recovery::Snapshot =
        dmx_protocol::decode_strict(i.document().as_bytes(), 256 * 1024).unwrap();
    s.revision = Revision(u64::MAX);
    let document = s.encode().unwrap();
    assert!(Installation::restore(document.as_bytes()).is_err());
    s.revision = Revision(u64::MAX - 1);
    let mut restored = Installation::restore(s.encode().unwrap().as_bytes()).unwrap();
    for _ in 0..4 {
        restored.tick().unwrap();
    }
    assert_eq!(restored.revision(), Revision(u64::MAX));
    assert_eq!(restored.output(&id(2), &channel()), 255);
}

#[test]
fn stale_desired_is_rejected_before_attempting_to_resolve_its_old_effect() {
    let mut i = installation();
    let now = Instant::now();
    i.desired(&id(2), request(1), id(7), None, || now).unwrap();
    let command = Command {
        revision: Revision(4),
        expected_playback: Revision(1),
        expected_execution: id(7),
        history: id(4),
        authority: id(5),
        generation: id(3),
        restart: false,
    };
    i.command(&id(2), command, id(8), None, || now).unwrap();
    let mut old = request(2);
    old.effect = Some(id(99));
    let error = i.desired(&id(2), old, id(9), None, || now).unwrap_err();
    assert!(
        error.to_string().contains("superseded"),
        "expected ordering rejection, got {error}"
    );
}

#[test]
fn completion_growth_is_reserved_before_accepting_a_running_snapshot() {
    use mqtt_dmx::{
        compiler::CompiledConfiguration, ordered_array::OrderedArray, recovery::Snapshot,
    };
    let mut c = config();
    c.content.arrays.get_mut(&id(2)).unwrap().lights.insert(
        "all".into(),
        (0..352)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(","),
    );
    c.digest = c.content.digest().unwrap();
    let compiled = CompiledConfiguration::compile(c.clone()).unwrap();
    let effect = compiled
        .effect(&id(2), None, &BTreeMap::new(), false, 200)
        .unwrap();
    let array = OrderedArray::new(
        compiled.ownership()[&id(2)].clone(),
        request(1),
        effect,
        id(7),
    )
    .unwrap();
    let initialized = Installation::initialize(c, BTreeMap::from([(id(2), id(6))])).unwrap();
    let mut s: Snapshot =
        dmx_protocol::decode_strict(initialized.document().as_bytes(), 256 * 1024).unwrap();
    s.arrays.insert(id(2), array.checkpoint().unwrap());
    // Find the largest current image that fits using the actual wire encoder.
    let (mut low, mut high) = (0usize, 120_000usize);
    while low < high {
        let mid = (low + high).div_ceil(2);
        s.configuration
            .content
            .universes
            .get_mut(&id(1))
            .unwrap()
            .description = "x".repeat(mid);
        s.configuration.digest = s.configuration.content.digest().unwrap();
        if s.encode().is_ok() {
            low = mid
        } else {
            high = mid - 1
        }
    }
    s.configuration
        .content
        .universes
        .get_mut(&id(1))
        .unwrap()
        .description = "x".repeat(low);
    s.configuration.digest = s.configuration.content.digest().unwrap();
    assert!(low > 0);
    s.encode().unwrap();
    assert!(
        s.encode_for_commit()
            .unwrap_err()
            .to_string()
            .contains("carrier limit"),
        "future held planes must fit before this execution is admitted"
    );
}

fn next_config(i: &Installation) -> Configuration {
    let mut c = i.configuration().clone();
    c.expected_previous = Some(c.revision);
    c.revision = c.revision.next().unwrap();
    c
}
fn rehash(c: &mut Configuration) {
    c.digest = c.content.digest().unwrap();
}

#[test]
fn definition_edit_and_rename_keep_running_timeline_and_execution() {
    let mut i = installation();
    let now = Instant::now();
    i.desired(&id(2), request(1), id(7), None, || now).unwrap();
    i.tick().unwrap();
    let mut c = next_config(&i);
    c.content.arrays.get_mut(&id(2)).unwrap().name = "renamed".into();
    c.content.values.insert("new".into(), "value".into());
    rehash(&mut c);
    assert!(i
        .configure(c, BTreeMap::new(), None, || now)
        .unwrap()
        .released
        .is_empty());
    i.tick().unwrap();
    assert_eq!(i.output(&id(2), &channel()), 128);
    assert_eq!(i.receipt(&id(2)).unwrap().execution, id(7));
}

#[test]
fn remapping_requires_completed_off_and_rotates_generation_before_any_commit() {
    let now = Instant::now();
    let mut i = installation();
    i.desired(&id(2), request(1), id(7), None, || now).unwrap();
    let mut c = next_config(&i);
    let a = c.content.arrays.get_mut(&id(2)).unwrap();
    a.lights.insert("all".into(), "9".into());
    a.generation = id(8);
    rehash(&mut c);
    let before = i.document();
    assert!(i
        .configure(c.clone(), BTreeMap::from([(id(2), id(9))]), None, || now)
        .is_err());
    assert_eq!(i.document(), before);
    let mut off = request(2);
    off.level = Level::OFF;
    off.fade_ms = 0;
    i.desired(&id(2), off, id(10), None, || now).unwrap();
    let commit = i
        .configure(c, BTreeMap::from([(id(2), id(9))]), None, || now)
        .unwrap();
    assert_eq!(commit.released, vec![channel()]);
    assert_eq!(i.output(&id(2), &channel()), 0);
    assert!(i
        .desired(&id(2), request(3), id(11), None, || now)
        .unwrap_err()
        .to_string()
        .contains("fenced"));
    let mut new = request(3);
    new.generation = id(8);
    i.desired(&id(2), new, id(11), None, || now).unwrap();
}

#[test]
fn powered_off_running_or_stopped_array_cannot_be_deleted() {
    let now = Instant::now();
    let mut i = installation();
    i.desired(&id(2), request(1), id(7), None, || now).unwrap();
    let mut off = request(2);
    off.level = Level::OFF;
    i.desired(&id(2), off, id(8), None, || now).unwrap();
    let mut c = next_config(&i);
    c.content.arrays.clear();
    rehash(&mut c);
    assert!(i
        .configure(c.clone(), BTreeMap::new(), None, || now)
        .is_err());
    let stop = Command {
        revision: Revision(3),
        expected_playback: Revision(2),
        expected_execution: id(8),
        history: id(4),
        authority: id(5),
        generation: id(3),
        restart: false,
    };
    i.command(&id(2), stop, id(9), None, || now).unwrap();
    assert!(i.configure(c, BTreeMap::new(), None, || now).is_err());
}

#[test]
fn removed_ids_and_old_mapping_generations_cannot_be_reused_in_same_history() {
    let now = Instant::now();
    let mut i = installation();
    let original = i.configuration().clone();
    let mut empty = next_config(&i);
    empty.content.arrays.clear();
    rehash(&mut empty);
    i.configure(empty, BTreeMap::new(), None, || now).unwrap();
    let mut resurrect = original;
    resurrect.expected_previous = Some(i.configuration().revision);
    resurrect.revision = i.configuration().revision.next().unwrap();
    assert!(i
        .configure(resurrect, BTreeMap::from([(id(2), id(8))]), None, || now)
        .is_err());
    let recovered = Installation::restore(i.document().as_bytes()).unwrap();
    assert!(recovered.receipt(&id(2)).is_none());
}

#[test]
fn stale_configuration_base_and_expiry_preserve_old_configuration() {
    let now = Instant::now();
    let mut i = installation();
    let mut c = next_config(&i);
    c.content.values.insert("test".into(), "1".into());
    rehash(&mut c);
    let before = i.document();
    c.expected_previous = Some(Revision(0));
    assert!(i
        .configure(c.clone(), BTreeMap::new(), None, || now)
        .is_err());
    c.expected_previous = Some(Revision(1));
    let deadline = now + Duration::from_secs(1);
    let calls = std::cell::Cell::new(0);
    assert!(i
        .configure(c, BTreeMap::new(), Some(deadline), || {
            let n = calls.get();
            calls.set(n + 1);
            if n == 0 {
                now
            } else {
                deadline
            }
        })
        .is_err());
    assert_eq!(i.document(), before);
}

#[test]
fn explicit_restart_uses_updated_definition_but_running_recovery_keeps_accepted_plan() {
    use dmx_protocol::configuration::{NamedEffect, Node, Ticks};
    let now = Instant::now();
    let mut c = config();
    let effect = |value: u8| NamedEffect {
        name: "custom".into(),
        description: "".into(),
        definition: Node::Fade {
            lights: "@all".into(),
            ticks: Ticks::Number(4),
            target: format!("s({value})"),
            no_dimming: false,
        },
    };
    c.content
        .arrays
        .get_mut(&id(2))
        .unwrap()
        .effects
        .insert(id(20), effect(200));
    rehash(&mut c);
    let mut i = Installation::initialize(c, BTreeMap::from([(id(2), id(6))])).unwrap();
    let mut p = request(1);
    p.effect = Some(id(20));
    i.desired(&id(2), p, id(7), None, || now).unwrap();
    i.tick().unwrap();
    let mut c = next_config(&i);
    c.content
        .arrays
        .get_mut(&id(2))
        .unwrap()
        .effects
        .insert(id(20), effect(240));
    rehash(&mut c);
    i.configure(c, BTreeMap::new(), None, || now).unwrap();
    let mut recovered = Installation::restore(i.document().as_bytes()).unwrap();
    recovered.tick().unwrap();
    assert_eq!(recovered.output(&id(2), &channel()), 50);
    let restart = Command {
        revision: Revision(2),
        expected_playback: Revision(1),
        expected_execution: id(7),
        history: id(4),
        authority: id(5),
        generation: id(3),
        restart: true,
    };
    i.command(&id(2), restart, id(8), None, || now).unwrap();
    i.tick().unwrap();
    assert_eq!(i.output(&id(2), &channel()), 60);
}

#[test]
fn identity_kind_cannot_change_by_reusing_a_removed_objects_uuid() {
    let now = Instant::now();
    let mut i = installation();
    let mut c = next_config(&i);
    let universe = c.content.universes.remove(&id(1)).unwrap();
    c.content.universes.insert(id(9), universe);
    let a = c.content.arrays.get_mut(&id(2)).unwrap();
    a.generation = id(1);
    a.lights.insert("all".into(), "9".into());
    rehash(&mut c);
    assert!(i
        .configure(c, BTreeMap::from([(id(2), id(8))]), None, || now)
        .is_err());
}

#[test]
fn generation_rotates_exactly_when_physical_ownership_changes() {
    let now = Instant::now();
    let mut i = installation();
    let mut c = next_config(&i);
    c.content.arrays.get_mut(&id(2)).unwrap().generation = id(9);
    rehash(&mut c);
    assert!(i.configure(c, BTreeMap::new(), None, || now).is_err());
    let mut c = next_config(&i);
    c.content
        .arrays
        .get_mut(&id(2))
        .unwrap()
        .lights
        .insert("all".into(), "9".into());
    rehash(&mut c);
    assert!(i
        .configure(c.clone(), BTreeMap::from([(id(2), id(8))]), None, || now)
        .is_err());
    c.content.arrays.get_mut(&id(2)).unwrap().generation = id(9);
    rehash(&mut c);
    i.configure(c, BTreeMap::from([(id(2), id(8))]), None, || now)
        .unwrap();
}
