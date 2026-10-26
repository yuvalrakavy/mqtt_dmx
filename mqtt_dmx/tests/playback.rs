use dmx_protocol::{ChannelPlanes, Level};
use mqtt_dmx::engine::{Channel, Effect, Player, Status, Target};

fn channel(number: u16) -> Channel {
    Channel {
        endpoint: "127.0.0.1:6454".parse().unwrap(),
        net: 0,
        subnet: 0,
        universe: 0,
        channel: number,
    }
}
fn planes(value: u8) -> ChannelPlanes {
    ChannelPlanes {
        dimmable: value,
        independent: 0,
    }
}
fn fade(number: u16, value: u8, ticks: u32) -> Effect {
    Effect::Fade {
        ticks,
        targets: vec![Target {
            channel: channel(number),
            planes: planes(value),
        }],
    }
}
fn player(effect: Effect) -> Player {
    Player::start([channel(0)], effect, vec![], Level::FULL).unwrap()
}

#[test]
fn running_restore_restarts_but_completed_restore_preserves_final_output() {
    let mut original = player(fade(0, 200, 4));
    original.tick();
    original.tick();
    assert_eq!(original.output(&channel(0)), 100);
    let mut restarted = Player::restore([channel(0)], original.snapshot()).unwrap();
    assert_eq!(restarted.output(&channel(0)), 0);
    restarted.tick();
    assert_eq!(restarted.output(&channel(0)), 50);
    original.tick();
    original.tick();
    assert_eq!(original.status(), Status::Completed);
    let mut completed = Player::restore([channel(0)], original.snapshot()).unwrap();
    assert_eq!(completed.status(), Status::Completed);
    completed.tick();
    assert_eq!(completed.output(&channel(0)), 200);
}

#[test]
fn stopped_restore_keeps_frozen_output_and_never_ticks_again() {
    let mut original = player(fade(0, 200, 4));
    original.tick();
    original.stop();
    let mut restored = Player::restore([channel(0)], original.snapshot()).unwrap();
    for _ in 0..20 {
        restored.tick();
    }
    assert_eq!(restored.status(), Status::Stopped);
    assert_eq!(restored.output(&channel(0)), 50);
}

#[test]
fn live_brightness_changes_preserve_timeline_and_zero_does_not_erase_planes() {
    let mut value = player(fade(0, 200, 4));
    value.tick();
    value.set_level(Level::try_from(500).unwrap());
    assert_eq!(value.output(&channel(0)), 25);
    value.tick();
    assert_eq!(value.output(&channel(0)), 50);
    value.set_level(Level::OFF);
    assert_eq!(value.output(&channel(0)), 0);
    value.set_level(Level::FULL);
    assert_eq!(value.output(&channel(0)), 100);
    value.tick();
    value.tick();
    assert_eq!(value.status(), Status::Completed);
    assert_eq!(value.output(&channel(0)), 200);
}

#[test]
fn fade_between_dimming_modes_clears_the_other_plane() {
    let initial = vec![Target {
        channel: channel(0),
        planes: ChannelPlanes {
            dimmable: 0,
            independent: 200,
        },
    }];
    let mut value = Player::start(
        [channel(0)],
        fade(0, 200, 2),
        initial,
        Level::try_from(500).unwrap(),
    )
    .unwrap();
    assert_eq!(value.output(&channel(0)), 200);
    value.tick();
    assert_eq!(value.output(&channel(0)), 150);
    value.tick();
    assert_eq!(value.output(&channel(0)), 100);
}

#[test]
fn sequences_and_parallel_writes_have_deterministic_boundaries() {
    let mut sequential = player(Effect::Sequence {
        nodes: vec![fade(0, 100, 0), Effect::Delay { ticks: 2 }, fade(0, 200, 0)],
    });
    assert_eq!(
        sequential.output(&channel(0)),
        100,
        "instant setup settles at start"
    );
    sequential.tick();
    assert_eq!(sequential.output(&channel(0)), 100);
    sequential.tick();
    assert_eq!(sequential.status(), Status::Completed);
    assert_eq!(sequential.output(&channel(0)), 200);
    let mut parallel = player(Effect::Parallel {
        nodes: vec![fade(0, 100, 2), fade(0, 200, 2)],
    });
    parallel.tick();
    parallel.tick();
    assert_eq!(
        parallel.output(&channel(0)),
        200,
        "later declared write wins"
    );
}

#[test]
fn ownership_is_checked_for_every_nested_branch_before_starting() {
    let illegal = Effect::Sequence {
        nodes: vec![
            fade(0, 200, 2),
            Effect::Parallel {
                nodes: vec![fade(1, 200, 1)],
            },
        ],
    };
    assert!(Player::start([channel(0)], illegal, vec![], Level::FULL).is_err());
    let forged = vec![Target {
        channel: channel(1),
        planes: planes(200),
    }];
    assert!(Player::start([channel(0)], fade(0, 200, 1), forged, Level::FULL).is_err());
}

#[test]
fn physical_aliases_are_normalized_and_invalid_addresses_refuse() {
    let mut alias = channel(0);
    alias.endpoint = "[::ffff:127.0.0.1]:6454".parse().unwrap();
    let value = Player::start(
        [channel(0)],
        Effect::Fade {
            ticks: 0,
            targets: vec![Target {
                channel: alias,
                planes: planes(123),
            }],
        },
        vec![],
        Level::FULL,
    )
    .unwrap();
    assert_eq!(value.output(&channel(0)), 123);
    let mut invalid = channel(512);
    assert!(Player::start(
        [invalid.clone()],
        Effect::Delay { ticks: 0 },
        vec![],
        Level::FULL
    )
    .is_err());
    invalid.channel = 0;
    invalid.net = 128;
    assert!(Player::start([invalid], Effect::Delay { ticks: 0 }, vec![], Level::FULL).is_err());
}

#[test]
fn damaged_checkpoint_cannot_silently_drop_owned_output() {
    let mut value = player(fade(0, 200, 1));
    value.tick();
    let mut snapshot = value.snapshot();
    snapshot.held.clear();
    assert!(Player::restore([channel(0)], snapshot).is_err());
    let mut snapshot = value.snapshot();
    snapshot.initial.clear();
    assert!(Player::restore([channel(0)], snapshot).is_err());
    let mut snapshot = value.snapshot();
    snapshot.held.push(snapshot.held[0].clone());
    assert!(Player::restore([channel(0)], snapshot).is_err());
}
