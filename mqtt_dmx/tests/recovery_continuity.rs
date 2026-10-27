use dmx_protocol::{
    configuration::{Array, Configuration, Content, Universe},
    Id, Level, Playback, Revision,
};
use mqtt_dmx::{
    engine::{Channel, Status},
    installation::Installation,
    ordered_array::Command,
};
use std::{collections::BTreeMap, time::Instant};

fn id(value: u8) -> Id {
    format!("00000000-0000-0000-0000-{value:012}")
        .try_into()
        .unwrap()
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
fn installation() -> Installation {
    let mut content = Content::default();
    content.universes.insert(
        id(1),
        Universe {
            name: "main".into(),
            description: String::new(),
            endpoint: channel().endpoint,
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
            description: String::new(),
            generation: id(3),
            universe: "main".into(),
            lights: [("all".into(), "1".into())].into(),
            on: None,
            off: None,
            effects: BTreeMap::new(),
            defaults: BTreeMap::new(),
        },
    );
    let configuration = Configuration {
        schema: 1,
        history: id(4),
        authority: id(5),
        revision: Revision(1),
        expected_previous: None,
        digest: content.digest().unwrap(),
        content,
    };
    let mut installation =
        Installation::initialize(configuration, [(id(2), id(6))].into()).unwrap();
    installation
        .desired(
            &id(2),
            Playback {
                schema: 1,
                history: id(4),
                authority: id(5),
                revision: Revision(1),
                generation: id(3),
                level: Level::FULL,
                effect: None,
                parameters: BTreeMap::new(),
                off_effect: None,
                fade_ms: 200,
            },
            id(7),
            None,
            Instant::now,
        )
        .unwrap();
    installation
}

#[test]
fn several_local_completions_publish_one_cut_above_both_revisions() {
    let mut live = installation();
    let mut configuration = live.configuration().clone();
    configuration.revision = Revision(2);
    configuration.expected_previous = Some(Revision(1));
    for (array, generation, channel) in [(10, 12, "2"), (11, 13, "3")] {
        let mut definition = configuration.content.arrays[&id(2)].clone();
        definition.name = format!("array{array}");
        definition.generation = id(generation);
        definition.lights = [("all".into(), channel.into())].into();
        configuration.content.arrays.insert(id(array), definition);
    }
    configuration.digest = configuration.content.digest().unwrap();
    live.configure(
        configuration,
        [(id(10), id(14)), (id(11), id(15))].into(),
        None,
        Instant::now,
    )
    .unwrap();
    for (array, generation, execution, fade_ms) in [(10, 12, 16, 50), (11, 13, 17, 100)] {
        live.desired(
            &id(array),
            Playback {
                schema: 1,
                history: id(4),
                authority: id(5),
                revision: Revision(1),
                generation: id(generation),
                level: Level::FULL,
                effect: None,
                parameters: BTreeMap::new(),
                off_effect: None,
                fade_ms,
            },
            id(execution),
            None,
            Instant::now,
        )
        .unwrap();
    }
    let mut broker = Installation::restore(live.document().as_bytes()).unwrap();
    let mut renamed = broker.configuration().clone();
    renamed.revision = Revision(3);
    renamed.expected_previous = Some(Revision(2));
    renamed.content.arrays.get_mut(&id(2)).unwrap().name = "renamed".into();
    renamed.digest = renamed.content.digest().unwrap();
    broker
        .configure(renamed, BTreeMap::new(), None, Instant::now)
        .unwrap();
    for _ in 0..4 {
        live.tick().unwrap();
    }
    assert!(
        live.revision() > broker.revision(),
        "fixture must distinguish the two revision bases"
    );
    let expected = live.revision().next().unwrap();
    let selected = broker.preserve_running_from(&live).unwrap();
    assert_eq!(
        selected.revision(),
        expected,
        "multiple completions must publish one cut above both bases"
    );
    for array in [2, 10, 11] {
        assert_eq!(
            selected.receipt(&id(array)).unwrap().status,
            Status::Completed
        );
    }
    let restored = Installation::restore(selected.document().as_bytes()).unwrap();
    assert_eq!(restored.revision(), expected);
}

#[test]
fn stopped_local_playback_is_not_mistaken_for_a_natural_completion() {
    let mut live = installation();
    let broker = Installation::restore(live.document().as_bytes()).unwrap();
    live.tick().unwrap();
    live.command(
        &id(2),
        Command {
            revision: Revision(2),
            expected_playback: Revision(1),
            expected_execution: id(7),
            history: id(4),
            authority: id(5),
            generation: id(3),
            restart: false,
        },
        id(8),
        None,
        Instant::now,
    )
    .unwrap();
    let revision = broker.revision();
    let selected = broker.preserve_running_from(&live).unwrap();
    assert_eq!(selected.receipt(&id(2)).unwrap().status, Status::Running);
    assert_eq!(selected.revision(), revision);
}

#[test]
fn locally_completed_execution_is_promoted_above_the_newer_broker_cut_once() {
    let mut live = installation();
    let base = live.document();
    let mut broker = Installation::restore(base.as_bytes()).unwrap();
    let mut renamed = broker.configuration().clone();
    renamed.revision = Revision(2);
    renamed.expected_previous = Some(Revision(1));
    renamed.content.arrays.get_mut(&id(2)).unwrap().name = "renamed".into();
    renamed.digest = renamed.content.digest().unwrap();
    broker
        .configure(renamed, BTreeMap::new(), None, Instant::now)
        .unwrap();
    assert!(broker.revision() > Installation::restore(base.as_bytes()).unwrap().revision());
    for _ in 0..4 {
        live.tick().unwrap();
    }
    assert_eq!(live.receipt(&id(2)).unwrap().status, Status::Completed);
    let expected_revision = std::cmp::max(live.revision(), broker.revision())
        .next()
        .unwrap();
    let selected = broker.preserve_running_from(&live).unwrap();
    assert_eq!(
        selected.receipt(&id(2)).unwrap().status,
        Status::Completed,
        "newer broker cut restarted a locally completed matching execution"
    );
    assert_eq!(selected.output(&id(2), &channel()), 255);
    assert_eq!(
        selected.configuration().content.arrays[&id(2)].name,
        "renamed"
    );
    assert_eq!(
        selected.revision(),
        expected_revision,
        "completion promotion did not advance the selected cut exactly once"
    );
    let document = selected.document();
    let mut restored = Installation::restore(document.as_bytes())
        .unwrap()
        .preserve_running_from(&live)
        .unwrap();
    for _ in 0..10 {
        restored.tick().unwrap();
    }
    assert_eq!(
        restored.document(),
        document,
        "a terminal cut was promoted again"
    );
    assert_eq!(restored.output(&id(2), &channel()), 255);
}

#[test]
fn local_completion_cannot_override_a_different_broker_level() {
    let mut live = installation();
    let mut broker = Installation::restore(live.document().as_bytes()).unwrap();
    let snapshot: mqtt_dmx::recovery::Snapshot = serde_json::from_str(&broker.document()).unwrap();
    let mut playback = Playback::decode(snapshot.arrays[&id(2)].playback.as_bytes()).unwrap();
    playback.revision = Revision(2);
    playback.level = Level::try_from(500).unwrap();
    broker
        .desired(&id(2), playback, id(8), None, Instant::now)
        .unwrap();
    let revision = broker.revision();
    for _ in 0..4 {
        live.tick().unwrap();
    }
    let selected = broker.preserve_running_from(&live).unwrap();
    assert_eq!(selected.receipt(&id(2)).unwrap().status, Status::Running);
    assert_eq!(selected.output(&id(2), &channel()), 0);
    assert_eq!(
        selected.revision(),
        revision,
        "unmatched local completion advanced the broker cut"
    );
}

#[test]
fn matching_running_broker_cut_preserves_position_and_next_tick() {
    let mut live = installation();
    let cut = live.document();
    live.tick().unwrap();
    live.tick().unwrap();
    let level = live.output(&id(2), &channel());
    assert!(level > 0 && level < 255);
    let mut selected = Installation::restore(cut.as_bytes())
        .unwrap()
        .preserve_running_from(&live)
        .unwrap();
    assert_eq!(
        selected.output(&id(2), &channel()),
        level,
        "adoption restarted a matching execution"
    );
    assert_eq!(
        selected.document(),
        cut,
        "continuity changed the broker cut"
    );
    selected.tick().unwrap();
    live.tick().unwrap();
    assert_eq!(
        selected.output(&id(2), &channel()),
        live.output(&id(2), &channel())
    );
}

#[test]
fn completed_or_stopped_broker_output_replaces_running_fallback() {
    for stopped in [false, true] {
        let mut broker = installation();
        broker.tick().unwrap();
        if stopped {
            broker
                .command(
                    &id(2),
                    Command {
                        revision: Revision(2),
                        expected_playback: Revision(1),
                        expected_execution: id(7),
                        history: id(4),
                        authority: id(5),
                        generation: id(3),
                        restart: false,
                    },
                    id(8),
                    None,
                    Instant::now,
                )
                .unwrap();
        } else {
            for _ in 0..3 {
                broker.tick().unwrap();
            }
        }
        let expected = broker.output(&id(2), &channel());
        assert!(
            expected > 0,
            "terminal fixture must exercise a lit owned channel"
        );
        let expected_status = if stopped {
            Status::Stopped
        } else {
            Status::Completed
        };
        let live = installation();
        let mut selected = Installation::restore(broker.document().as_bytes())
            .unwrap()
            .preserve_running_from(&live)
            .unwrap();
        assert_eq!(selected.receipt(&id(2)).unwrap().status, expected_status);
        for _ in 0..10 {
            selected.tick().unwrap();
        }
        assert_eq!(
            selected.output(&id(2), &channel()),
            expected,
            "terminal broker cut resumed an animation"
        );
    }
}

#[test]
fn a_different_execution_cannot_inherit_the_live_position() {
    let mut live = installation();
    live.tick().unwrap();
    live.tick().unwrap();
    let mut broker = installation();
    broker
        .command(
            &id(2),
            Command {
                revision: Revision(2),
                expected_playback: Revision(1),
                expected_execution: id(7),
                history: id(4),
                authority: id(5),
                generation: id(3),
                restart: true,
            },
            id(9),
            None,
            Instant::now,
        )
        .unwrap();
    let selected = Installation::restore(broker.document().as_bytes())
        .unwrap()
        .preserve_running_from(&live)
        .unwrap();
    assert_eq!(
        selected.output(&id(2), &channel()),
        0,
        "different execution borrowed a live timeline"
    );
    assert_eq!(selected.receipt(&id(2)).unwrap().execution, id(9));
}
