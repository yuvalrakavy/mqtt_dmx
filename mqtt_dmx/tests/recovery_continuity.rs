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
