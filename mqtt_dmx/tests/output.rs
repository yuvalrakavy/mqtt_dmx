use dmx_protocol::{
    configuration::{Array, Configuration, Content, Universe},
    Id, Level, Playback, Revision,
};
use mqtt_dmx::{engine::Channel, installation::Installation, output::Frames};
use std::{collections::BTreeMap, time::Instant};

fn id(n: u8) -> Id {
    format!("00000000-0000-0000-0000-{n:012}")
        .try_into()
        .unwrap()
}
fn installation(mapped_ipv4: bool) -> Installation {
    installation_at(mapped_ipv4, 9)
}
fn installation_at(mapped_ipv4: bool, first_net: u8) -> Installation {
    installation_with_extra(mapped_ipv4, first_net, false)
}
fn installation_with_extra(mapped_ipv4: bool, first_net: u8, extra: bool) -> Installation {
    let mut content = Content::default();
    for (u, a, generation, name, net, subnet, universe) in [
        (1, 2, 3, "first", first_net, 3, 5),
        (8, 9, 10, "second", 2, 7, 11),
    ] {
        content.universes.insert(
            id(u),
            Universe {
                name: name.into(),
                description: String::new(),
                endpoint: if mapped_ipv4 {
                    "[::ffff:127.0.0.1]:6454"
                } else {
                    "127.0.0.1:6454"
                }
                .parse()
                .unwrap(),
                net,
                subnet,
                universe,
                channels: 512,
            },
        );
        content.arrays.insert(
            id(a),
            Array {
                name: name.into(),
                description: String::new(),
                generation: id(generation),
                universe: name.into(),
                lights: BTreeMap::from([("all".into(), "1,3".into())]),
                on: None,
                off: None,
                effects: BTreeMap::new(),
                defaults: BTreeMap::new(),
            },
        );
    }
    if extra {
        let mut unused = content.universes[&id(1)].clone();
        unused.name = "unused".into();
        unused.net = 99;
        content.universes.insert(id(30), unused);
    }
    let configuration = Configuration {
        schema: 1,
        history: id(4),
        authority: id(5),
        revision: Revision(1),
        expected_previous: None,
        digest: content.digest().unwrap(),
        content,
    };
    let mut installation = Installation::initialize(
        configuration,
        BTreeMap::from([(id(2), id(6)), (id(9), id(11))]),
    )
    .unwrap();
    for (array, generation, execution, level) in [(2, 3, 7, 500), (9, 10, 12, 1000)] {
        let request = Playback {
            schema: 1,
            history: id(4),
            authority: id(5),
            revision: Revision(1),
            generation: id(generation),
            level: Level::try_from(level).unwrap(),
            effect: None,
            parameters: BTreeMap::new(),
            off_effect: None,
            fade_ms: 0,
        };
        installation
            .desired(&id(array), request, id(execution), None, Instant::now)
            .unwrap();
    }
    installation
}

#[test]
fn output_uses_engine_values_and_the_exact_universe_address_on_each_packet() {
    let installation = installation(false);
    let mut frames = Frames::new(&installation).unwrap();
    frames.render(&installation).unwrap();
    let packets = frames.datagrams();
    assert_eq!(packets.len(), 2, "output lost a configured universe");
    for packet in packets {
        assert_eq!(packet.destination, "127.0.0.1:6454".parse().unwrap());
        assert_eq!(&packet.bytes[..12], b"Art-Net\0\x00\x50\x00\x0e");
        assert_eq!(&packet.bytes[16..18], &[2, 0]);
        assert_eq!(packet.bytes.len(), 530);
        assert_eq!(packet.bytes[13], 0);
        let (address, value) = if packet.bytes[15] == 9 {
            (0x35, 128)
        } else {
            assert_eq!(packet.bytes[15], 2);
            (0x7b, 255)
        };
        assert_eq!(
            packet.bytes[14], address,
            "output swapped net/subnet/universe addressing"
        );
        let mut expected = [0u8; 512];
        expected[1] = value;
        expected[3] = value;
        assert_eq!(
            &packet.bytes[18..],
            &expected,
            "output diverged from actual engine channel values"
        );
    }
}

#[test]
fn every_tick_has_a_nonzero_sequence_and_wrapping_does_not_disable_ordering() {
    let installation = installation(false);
    let mut frames = Frames::new(&installation).unwrap();
    for tick in 0..260 {
        let packets = frames.datagrams();
        assert_eq!(packets.len(), 2, "tick produced no Art-Net frames");
        for packet in packets {
            assert_eq!(
                packet.bytes[12],
                (tick % 255 + 1) as u8,
                "sequence wrapped through disabled value zero"
            );
        }
    }
}

#[test]
fn released_output_is_cleared_without_erasing_other_channels() {
    let installation = installation(false);
    let mut frames = Frames::new(&installation).unwrap();
    let released: Vec<Channel> = installation
        .channels()
        .filter(|(array, channel)| **array == id(2) && channel.channel == 1)
        .map(|(_, channel)| channel.clone())
        .collect();
    assert_eq!(released.len(), 1);
    let packets = frames.clear_released(&released).unwrap();
    assert_eq!(
        packets.len(),
        1,
        "released universe did not receive a clearing frame"
    );
    assert_eq!(packets[0].bytes[15], 9);
    assert_eq!(
        packets[0].bytes[19], 0,
        "released channel kept its old lighting value"
    );
    assert_eq!(
        packets[0].bytes[21], 128,
        "clearing released ownership erased an unrelated channel"
    );
}

#[test]
fn output_normalizes_ipv4_mapped_endpoints_like_the_ownership_compiler() {
    let installation = installation(true);
    let mut frames = Frames::new(&installation).unwrap();
    frames.render(&installation).unwrap();
    let packets = frames.datagrams();
    assert_eq!(
        packets.len(),
        2,
        "canonical endpoint lookup lost the compiled ownership"
    );
    assert!(packets.iter().all(|packet| packet.destination.is_ipv4()));
}

#[test]
fn invalid_release_does_not_partially_clear_previous_output() {
    let installation = installation(false);
    let mut frames = Frames::new(&installation).unwrap();
    let known = installation
        .channels()
        .find(|(array, channel)| **array == id(2) && channel.channel == 1)
        .unwrap()
        .1
        .clone();
    let mut missing = known.clone();
    missing.net = 99;
    assert!(frames.clear_released(&[known, missing]).is_err());
    let packet = frames
        .datagrams()
        .into_iter()
        .find(|packet| packet.bytes[15] == 9)
        .unwrap();
    assert_eq!(
        packet.bytes[19], 128,
        "invalid release partially cleared the old output"
    );
}

#[test]
fn a_configuration_mismatch_preserves_the_previous_frame_set() {
    let first = installation(false);
    let mut frames = Frames::new(&first).unwrap();
    for proposed in [
        installation_at(false, 10),
        installation_with_extra(false, 9, true),
    ] {
        assert!(
            frames.render(&proposed).is_err(),
            "changed universe mapping was rendered without activation"
        );
    }
    let packets = frames.datagrams();
    assert_eq!(packets.len(), 2);
    let packet = packets
        .iter()
        .find(|packet| packet.bytes[15] == 9)
        .expect("old universe disappeared after a refused render");
    assert_eq!(packet.bytes[19], 128);
    assert!(packets.iter().all(|packet| packet.bytes[15] != 10));
}

#[tokio::test]
async fn nonblocking_sender_delivers_an_actual_frame_to_an_owned_loopback_receiver() {
    let receiver = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let installation = installation(false);
    let mut frames = Frames::new(&installation).unwrap();
    let mut packet = frames.datagrams().remove(0);
    packet.destination = receiver.local_addr().unwrap();
    mqtt_dmx::output::DatagramSender::default()
        .send(&packet)
        .unwrap();
    let mut bytes = [0u8; 1024];
    let (length, source) = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        receiver.recv_from(&mut bytes),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(source.ip().is_loopback());
    assert_eq!(
        &bytes[..length],
        packet.bytes.as_slice(),
        "UDP delivery changed the produced Art-Net frame"
    );
}

#[test]
fn configuration_changes_preserve_sequences_for_unchanged_physical_universes() {
    let mut installation = installation(false);
    let mut frames = Frames::new(&installation).unwrap();
    for _ in 0..17 {
        assert_eq!(frames.datagrams().len(), 2);
    }
    let mut configuration = installation.configuration().clone();
    configuration.content.arrays.get_mut(&id(2)).unwrap().name = "renamed".into();
    configuration.revision = Revision(2);
    configuration.expected_previous = Some(Revision(1));
    configuration.digest = configuration.content.digest().unwrap();
    let commit = installation
        .configure(configuration, BTreeMap::new(), None, Instant::now)
        .unwrap();
    assert!(commit.released.is_empty());
    assert!(frames
        .reconfigure(&installation, &commit.released)
        .unwrap()
        .is_empty());
    let packets = frames.datagrams();
    assert_eq!(packets.len(), 2);
    for packet in packets {
        assert_eq!(
            packet.bytes[12], 18,
            "configuration activation restarted a live universe sequence"
        );
    }
}

#[test]
fn reconfiguration_returns_old_mapping_clear_frames_before_new_mapping_frames() {
    let installation = installation(false);
    let mut frames = Frames::new(&installation).unwrap();
    frames.datagrams();
    let released: Vec<_> = installation
        .channels()
        .filter(|(array, _)| **array == id(2))
        .map(|(_, channel)| channel.clone())
        .collect();
    assert_eq!(released.len(), 2);
    let clearing = frames
        .reconfigure(&installation_at(false, 10), &released)
        .unwrap();
    assert_eq!(clearing.len(), 1);
    assert_eq!(
        clearing[0].bytes[15], 9,
        "clearing targeted the new mapping instead of the old mapping"
    );
    assert_eq!(&clearing[0].bytes[18..], &[0u8; 512]);
    let next = frames.datagrams();
    assert_eq!(next.len(), 2);
    assert!(next.iter().all(|packet| packet.bytes[15] != 9));
    let preserved = next.iter().find(|packet| packet.bytes[15] == 2).unwrap();
    assert_eq!(
        preserved.bytes[12], 2,
        "unaffected universe sequence was reset by another universe remap"
    );
    assert_eq!(preserved.bytes[19], 255);
    assert_eq!(
        next.iter()
            .find(|packet| packet.bytes[15] == 10)
            .unwrap()
            .bytes[19],
        128
    );
}
