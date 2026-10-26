use dmx_protocol::{ChannelPlanes, Id, Level, Playback, Revision};
use mqtt_dmx::{
    engine::{Channel, Effect, Status, Target},
    ordered_array::{Command, OrderedArray},
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
fn channel() -> Channel {
    Channel {
        endpoint: "127.0.0.1:6454".parse().unwrap(),
        net: 0,
        subnet: 0,
        universe: 0,
        channel: 0,
    }
}
fn effect(value: u8) -> Effect {
    Effect::Fade {
        ticks: 4,
        targets: vec![Target {
            channel: channel(),
            planes: ChannelPlanes {
                dimmable: value,
                independent: 0,
            },
        }],
    }
}
fn playback(revision: u64) -> Playback {
    Playback {
        schema: 1,
        history: id(1),
        authority: id(2),
        generation: id(3),
        revision: Revision(revision),
        level: Level::FULL,
        effect: Some(id(4)),
        parameters: BTreeMap::new(),
        off_effect: None,
        fade_ms: 0,
    }
}
fn array() -> OrderedArray {
    OrderedArray::new(vec![channel()], playback(1), effect(200), id(5)).unwrap()
}
fn command(revision: u64, restart: bool) -> Command {
    Command {
        revision: Revision(revision),
        expected_playback: Revision(1),
        expected_execution: id(5),
        history: id(1),
        authority: id(2),
        generation: id(3),
        restart,
    }
}

#[test]
fn stop_supersedes_older_desired_and_duplicate_returns_original_receipt() {
    let now = Instant::now();
    let mut a = array();
    a.tick();
    let request = command(3, false);
    let stopped = a.command(request.clone(), id(6), None, now).unwrap();
    assert_eq!(stopped.status, Status::Stopped);
    assert!(a
        .desired(playback(2), effect(255), id(7), None, now)
        .is_err());
    assert_eq!(a.command(request, id(8), None, now).unwrap(), stopped);
    a.tick();
    assert_eq!(a.output(&channel()), 50);
}

#[test]
fn newer_equal_and_brightness_requests_preserve_execution_and_timeline() {
    let now = Instant::now();
    let mut a = array();
    a.tick();
    let receipt = a
        .desired(playback(2), effect(255), id(6), None, now)
        .unwrap();
    assert_eq!(receipt.execution, id(5));
    let mut request = playback(3);
    request.level = Level::try_from(500).unwrap();
    assert_eq!(
        a.desired(request, effect(255), id(7), None, now)
            .unwrap()
            .execution,
        id(5)
    );
    a.tick();
    assert_eq!(a.output(&channel()), 50);
}

#[test]
fn expiry_and_fencing_refuse_without_advancing_high_water_or_output() {
    let now = Instant::now();
    let mut a = array();
    a.tick();
    assert!(a
        .desired(playback(9), effect(255), id(6), Some(now), now)
        .is_err());
    for field in 0..3 {
        let mut p = playback(9);
        match field {
            0 => p.history = id(8),
            1 => p.authority = id(8),
            _ => p.generation = id(8),
        };
        assert!(a.desired(p, effect(255), id(6), None, now).is_err());
    }
    let accepted = a
        .desired(
            playback(2),
            effect(200),
            id(6),
            Some(now + Duration::from_secs(1)),
            now,
        )
        .unwrap();
    assert_eq!(accepted.revision, Revision(2));
    assert_eq!(a.output(&channel()), 50);
}

#[test]
fn conflicting_equal_revision_and_stale_command_guards_do_not_mutate() {
    let now = Instant::now();
    let mut a = array();
    a.desired(playback(2), effect(200), id(6), None, now)
        .unwrap();
    let mut p = playback(2);
    p.parameters.insert("color".into(), "red".into());
    assert!(a.desired(p, effect(255), id(7), None, now).is_err());
    assert!(a.command(command(3, false), id(8), None, now).is_err());
    assert_eq!(a.receipt().revision, Revision(2));
    assert_eq!(a.receipt().status, Status::Running);
}

#[test]
fn restart_uses_original_start_and_new_selection_uses_current_planes() {
    let now = Instant::now();
    let mut a = array();
    a.tick();
    a.tick();
    let request = command(2, true);
    let r = a.command(request.clone(), id(6), None, now).unwrap();
    assert_eq!(r.execution, id(6));
    assert_eq!(a.output(&channel()), 0);
    assert_eq!(a.command(request, id(7), None, now).unwrap(), r);
    a.tick();
    let mut p = playback(3);
    p.effect = Some(id(9));
    assert_eq!(
        a.desired(p, effect(250), id(8), None, now)
            .unwrap()
            .execution,
        id(8)
    );
    assert_eq!(a.output(&channel()), 50);
    a.tick();
    assert_eq!(a.output(&channel()), 100);
}

#[test]
fn invalid_effect_is_rejected_before_commit_and_off_selection_does_not_start() {
    let now = Instant::now();
    let mut a = array();
    a.tick();
    let mut p = playback(2);
    p.effect = Some(id(8));
    let mut bad = channel();
    bad.channel = 1;
    let invalid = Effect::Fade {
        ticks: 1,
        targets: vec![Target {
            channel: bad,
            planes: ChannelPlanes {
                dimmable: 255,
                independent: 0,
            },
        }],
    };
    assert!(a.desired(p, invalid, id(9), None, now).is_err());
    let mut off = playback(2);
    off.level = Level::OFF;
    a.desired(off.clone(), effect(0), id(6), None, now).unwrap();
    let before = a.receipt().execution;
    off.revision = Revision(3);
    off.effect = Some(id(9));
    assert_eq!(
        a.desired(off, effect(255), id(7), None, now)
            .unwrap()
            .execution,
        before
    );
    assert_eq!(a.output(&channel()), 0);
}

#[test]
fn restored_stop_keeps_high_water_and_original_duplicate_receipt() {
    let now = Instant::now();
    let mut a = array();
    a.tick();
    let stop = command(4, false);
    let receipt = a.command(stop.clone(), id(6), None, now).unwrap();
    let wire = serde_json::to_vec(&a.checkpoint().unwrap()).unwrap();
    let saved = dmx_protocol::decode_strict(&wire, 256 * 1024).unwrap();
    let mut recovered = OrderedArray::restore(vec![channel()], saved).unwrap();
    assert_eq!(recovered.output(&channel()), 50);
    assert!(recovered
        .desired(playback(3), effect(255), id(9), None, now)
        .is_err());
    assert_eq!(recovered.command(stop, id(8), None, now).unwrap(), receipt);
    recovered.tick();
    assert_eq!(recovered.output(&channel()), 50);
}

#[test]
fn restore_rejects_inconsistent_identity_level_and_operation_receipts() {
    let now = Instant::now();
    let mut a = array();
    a.command(command(2, false), id(6), None, now).unwrap();
    for change in 0..5 {
        let mut saved = a.checkpoint().unwrap();
        match change {
            0 => saved.receipt.execution = id(9),
            1 => saved.player.level = Level::OFF,
            2 => saved.receipt.revision = Revision(9),
            3 => saved.playback = playback(9).encode().unwrap(),
            _ => {
                if let mqtt_dmx::ordered_array::Operation::Command(ref mut c) = saved.last_operation
                {
                    c.history = id(9)
                }
            }
        };
        assert!(OrderedArray::restore(vec![channel()], saved).is_err());
    }
    let mut saved = a.checkpoint().unwrap();
    saved.player.held.clear();
    assert!(OrderedArray::restore(vec![channel()], saved).is_err());
}

#[test]
fn recovered_completed_execution_does_not_restart_on_equal_newer_desired() {
    let now = Instant::now();
    let mut a = array();
    for _ in 0..4 {
        a.tick();
    }
    let mut recovered = OrderedArray::restore(vec![channel()], a.checkpoint().unwrap()).unwrap();
    assert_eq!(
        recovered
            .desired(playback(2), effect(255), id(8), None, now)
            .unwrap()
            .status,
        Status::Completed
    );
    recovered.tick();
    assert_eq!(recovered.output(&channel()), 200);
}

#[test]
fn recovered_running_restarts_at_original_planes_and_retains_operation_revision() {
    let mut a = array();
    a.tick();
    a.tick();
    let mut recovered = OrderedArray::restore(vec![channel()], a.checkpoint().unwrap()).unwrap();
    assert_eq!(recovered.output(&channel()), 0);
    recovered.tick();
    assert_eq!(recovered.output(&channel()), 50);
    assert_eq!(recovered.receipt().revision, Revision(1));
}

#[test]
fn expired_command_and_reused_execution_identity_do_not_commit() {
    let now = Instant::now();
    let mut a = array();
    a.tick();
    assert!(a.command(command(2, false), id(6), Some(now), now).is_err());
    assert!(a.command(command(2, true), id(5), None, now).is_err());
    assert_eq!(a.receipt().revision, Revision(1));
    assert_eq!(a.output(&channel()), 50);
    a.command(command(2, false), id(6), None, now).unwrap();
}
