use dmx_protocol::{
    configuration::{Array, Configuration, Content, NamedEffect, Node, Ticks, Universe},
    Id, Level, Revision,
};
use mqtt_dmx::{compiler::CompiledConfiguration, engine::Player};
use std::collections::BTreeMap;
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
            name: "fixture".into(),
            description: "".into(),
            generation: id(3),
            universe: "main".into(),
            lights: BTreeMap::from([
                ("all".into(), "@white,@color,8".into()),
                ("white".into(), "w:1/4/7".into()),
                ("color".into(), "rgb:9".into()),
            ]),
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
fn named(node: Node) -> NamedEffect {
    NamedEffect {
        name: "custom".into(),
        description: "".into(),
        definition: node,
    }
}
fn rehash(c: &mut Configuration) {
    c.digest = c.content.digest().unwrap();
}

#[test]
fn fixed_ownership_and_builtin_targets_cover_single_rgb_and_white() {
    let c = CompiledConfiguration::compile(config()).unwrap();
    let owned = c.ownership()[&id(2)].clone();
    assert_eq!(owned.len(), 7);
    let effect = c.effect(&id(2), None, &BTreeMap::new(), false, 51).unwrap();
    let mut player = Player::start(owned.clone(), effect, vec![], Level::FULL).unwrap();
    player.tick();
    assert_eq!(player.output(&owned[0]), 128);
    player.tick();
    assert_eq!(player.output(&owned[0]), 255);
    let off = c.effect(&id(2), None, &BTreeMap::new(), true, 0).unwrap();
    let player = Player::start(owned.clone(), off, player.snapshot().held, Level::FULL).unwrap();
    for channel in owned {
        assert_eq!(player.output(&channel), 0);
    }
}

#[test]
fn expansion_uses_frozen_defaults_then_request_overrides_without_leaking_prior_values() {
    let mut c = config();
    c.content.values.insert("value".into(), "s(41)".into());
    let a = c.content.arrays.get_mut(&id(2)).unwrap();
    a.defaults.insert("value".into(), "s(73)".into());
    a.effects.insert(
        id(6),
        named(Node::Fade {
            lights: "`lights=8`".into(),
            ticks: Ticks::Parameter("`duration=0`".into()),
            target: "`value`".into(),
            no_dimming: false,
        }),
    );
    rehash(&mut c);
    let compiled = CompiledConfiguration::compile(c).unwrap();
    let owned = compiled.ownership()[&id(2)].clone();
    let custom = compiled
        .effect(
            &id(2),
            Some(&id(6)),
            &BTreeMap::from([("value".into(), "s(201)".into())]),
            false,
            0,
        )
        .unwrap();
    let p = Player::start(owned.clone(), custom, vec![], Level::FULL).unwrap();
    let ch = owned.iter().find(|c| c.channel == 8).unwrap();
    assert_eq!(p.output(ch), 201);
    let default = compiled
        .effect(&id(2), Some(&id(6)), &BTreeMap::new(), false, 0)
        .unwrap();
    assert_eq!(
        Player::start(owned.clone(), default, vec![], Level::FULL)
            .unwrap()
            .output(ch),
        73
    );
}

#[test]
fn probe_global_default_is_used_when_array_and_request_omit_it() {
    let mut c = config();
    c.content.values.insert("value".into(), "s(41)".into());
    c.content.arrays.get_mut(&id(2)).unwrap().effects.insert(
        id(6),
        named(Node::Fade {
            lights: "8".into(),
            ticks: Ticks::Number(0),
            target: "`value`".into(),
            no_dimming: false,
        }),
    );
    rehash(&mut c);
    let compiled = CompiledConfiguration::compile(c).unwrap();
    let owned = compiled.ownership()[&id(2)].clone();
    let effect = compiled
        .effect(&id(2), Some(&id(6)), &BTreeMap::new(), false, 0)
        .expect("global default must resolve the required value");
    let ch = owned.iter().find(|c| c.channel == 8).unwrap();
    assert_eq!(
        Player::start(owned.clone(), effect, vec![], Level::FULL)
            .unwrap()
            .output(ch),
        41
    );
}

#[test]
fn parameterized_raw_and_nested_effects_cannot_escape_published_all() {
    let mut c = config();
    c.content.arrays.get_mut(&id(2)).unwrap().effects.insert(
        id(6),
        named(Node::Parallel {
            nodes: vec![Node::Sequence {
                nodes: vec![Node::Fade {
                    lights: "`lights=8`".into(),
                    ticks: Ticks::Number(1),
                    target: "s(42)".into(),
                    no_dimming: false,
                }],
            }],
        }),
    );
    rehash(&mut c);
    let c = CompiledConfiguration::compile(c).unwrap();
    assert!(c
        .effect(
            &id(2),
            Some(&id(6)),
            &BTreeMap::from([("lights".into(), "12".into())]),
            false,
            0
        )
        .is_err());
    assert!(c
        .effect(
            &id(2),
            Some(&id(6)),
            &BTreeMap::from([("lights".into(), "$missing,8".into())]),
            false,
            0
        )
        .is_err());
}

#[test]
fn physical_aliases_overlap_between_arrays_but_groups_within_array_may_overlap() {
    let mut c = config();
    let mut second = c.content.arrays[&id(2)].clone();
    second.name = "second".into();
    second.generation = id(7);
    c.content.arrays.insert(id(6), second);
    rehash(&mut c);
    assert!(CompiledConfiguration::compile(c).is_err());
    let mut c = config();
    c.content
        .arrays
        .get_mut(&id(2))
        .unwrap()
        .lights
        .insert("all".into(), "8,8,@color,@color".into());
    rehash(&mut c);
    assert_eq!(
        CompiledConfiguration::compile(c).unwrap().ownership()[&id(2)].len(),
        4
    );
    let mut c = config();
    let mut alias = c.content.universes[&id(1)].clone();
    alias.name = "alias".into();
    alias.endpoint = "[::ffff:127.0.0.1]:6454".parse().unwrap();
    c.content.universes.insert(id(9), alias);
    rehash(&mut c);
    assert!(CompiledConfiguration::compile(c).is_err());
}

#[test]
fn circular_groups_out_of_range_channels_and_bad_targets_are_rejected() {
    for lights in ["@all", "rgb:511", "512", "s:1/2", "w:1/1/2", "$unknown,1"] {
        let mut c = config();
        c.content
            .arrays
            .get_mut(&id(2))
            .unwrap()
            .lights
            .insert("all".into(), lights.into());
        rehash(&mut c);
        assert!(CompiledConfiguration::compile(c).is_err(), "{lights}");
    }
    let mut c = config();
    c.content.arrays.get_mut(&id(2)).unwrap().effects.insert(
        id(6),
        named(Node::Fade {
            lights: "8".into(),
            ticks: Ticks::Number(0),
            target: "s(12)junk".into(),
            no_dimming: false,
        }),
    );
    rehash(&mut c);
    assert!(CompiledConfiguration::compile(c).is_err());
}

#[test]
fn strict_configuration_codec_checks_digest_and_complete_replacement() {
    let c = config();
    let encoded = c.encode().unwrap();
    assert_eq!(Configuration::decode(encoded.as_bytes()).unwrap(), c);
    let mut invalid = c.clone();
    invalid.content.values.insert("x".into(), "y".into());
    assert!(invalid.encode().is_err());
    let mut value = serde_json::to_value(c).unwrap();
    value.as_object_mut().unwrap().remove("expected_previous");
    assert!(Configuration::decode(&serde_json::to_vec(&value).unwrap()).is_err());
}

#[test]
fn required_request_parameters_are_supported_and_off_does_not_receive_on_overrides() {
    let mut c = config();
    let array = c.content.arrays.get_mut(&id(2)).unwrap();
    array.effects.insert(
        id(6),
        named(Node::Fade {
            lights: "8".into(),
            ticks: Ticks::Number(0),
            target: "`color`".into(),
            no_dimming: true,
        }),
    );
    array.effects.insert(
        id(7),
        named(Node::Fade {
            lights: "8".into(),
            ticks: Ticks::Number(0),
            target: "`color=s(0)`".into(),
            no_dimming: false,
        }),
    );
    array.off = Some(id(7));
    rehash(&mut c);
    let c = CompiledConfiguration::compile(c).unwrap();
    assert!(c
        .effect(&id(2), Some(&id(6)), &BTreeMap::new(), false, 0)
        .is_err());
    let values = BTreeMap::from([("color".into(), "s(210)".into())]);
    let owned = c.ownership()[&id(2)].clone();
    let channel = owned.iter().find(|c| c.channel == 8).unwrap();
    let effect = c.effect(&id(2), Some(&id(6)), &values, false, 0).unwrap();
    assert_eq!(
        Player::start(owned.clone(), effect, vec![], Level::try_from(100).unwrap())
            .unwrap()
            .output(channel),
        210
    );
    let off = c.effect(&id(2), None, &values, true, 0).unwrap();
    assert_eq!(
        Player::start(owned.clone(), off, vec![], Level::FULL)
            .unwrap()
            .output(channel),
        0
    );
}

#[test]
fn expanded_reference_budget_applies_across_all_effect_nodes() {
    let mut c = config();
    let a = c.content.arrays.get_mut(&id(2)).unwrap();
    let lights = std::iter::repeat_n("8", 128).collect::<Vec<_>>().join(",");
    a.effects.insert(
        id(6),
        named(Node::Sequence {
            nodes: (0..257)
                .map(|_| Node::Fade {
                    lights: lights.clone(),
                    ticks: Ticks::Number(0),
                    target: "s(9)".into(),
                    no_dimming: false,
                })
                .collect(),
        }),
    );
    rehash(&mut c);
    let compiled = CompiledConfiguration::compile(c).unwrap();
    assert!(compiled
        .effect(&id(2), Some(&id(6)), &BTreeMap::new(), false, 0)
        .is_err());
}

#[test]
fn unused_global_effect_is_resolved_only_in_the_array_that_executes_it() {
    let mut c = config();
    c.content.effects.insert(
        id(8),
        named(Node::Fade {
            lights: "@color".into(),
            ticks: Ticks::Number(0),
            target: "rgb(9,17,31)".into(),
            no_dimming: false,
        }),
    );
    let mut second = c.content.arrays[&id(2)].clone();
    second.name = "other".into();
    second.generation = id(9);
    second.lights = BTreeMap::from([("all".into(), "100".into())]);
    c.content.arrays.insert(id(7), second);
    rehash(&mut c);
    let compiled = CompiledConfiguration::compile(c).unwrap();
    assert!(compiled
        .effect(&id(2), Some(&id(8)), &BTreeMap::new(), false, 0)
        .is_ok());
    assert!(compiled
        .effect(&id(7), Some(&id(8)), &BTreeMap::new(), false, 0)
        .is_err());
}
