use dmx_protocol::{
    classify_operation, Admission, ChannelPlanes, Id, Level, OperationStamp, Playback, Revision,
    MAX_PLAYBACK_BYTES,
};
use std::collections::BTreeMap;

fn playback(level: u16) -> Playback {
    let id = || Id::try_from("11111111-1111-4111-8111-111111111111".to_owned()).unwrap();
    Playback {
        schema: 1,
        history: id(),
        authority: id(),
        revision: Revision(u64::MAX),
        generation: id(),
        level: Level::try_from(level).unwrap(),
        effect: None,
        parameters: BTreeMap::from([("colour".into(), "red".into())]),
        off_effect: None,
        fade_ms: 100,
    }
}

#[test]
fn level_is_the_only_power_authority() {
    for value in 0..=1000 {
        let level = Level::try_from(value).unwrap();
        assert_eq!(level.is_on(), value != 0, "level {value}");
        assert_eq!(serde_json::to_value(level).unwrap(), value);
    }
    assert!(Level::try_from(1001).is_err());
    assert!(serde_json::from_str::<Level>("-1").is_err());
    assert!(serde_json::from_str::<Level>("0.5").is_err());
}

#[test]
fn revision_is_lossless_and_never_wraps_or_stalls() {
    let max = Revision(u64::MAX);
    assert_eq!(
        serde_json::to_string(&max).unwrap(),
        "\"18446744073709551615\""
    );
    assert_eq!(Revision(7).next().unwrap(), Revision(8));
    assert!(max.next().is_err());
    for invalid in ["1", "-1", "\"01\"", "\"+1\"", "\"18446744073709551616\""] {
        assert!(
            serde_json::from_str::<Revision>(invalid).is_err(),
            "{invalid}"
        );
    }
}

#[test]
fn canonical_playback_roundtrips_and_fingerprints_the_exact_operation() {
    let original = playback(600);
    let text = original.encode().unwrap();
    assert!(
        text.starts_with("{\"authority\":"),
        "keys must be sorted: {text}"
    );
    assert_eq!(Playback::decode(text.as_bytes()).unwrap(), original);
    assert_eq!(original.fingerprint().unwrap().len(), 64);
    let mut next = original.clone();
    next.revision = Revision(1);
    assert_ne!(next.fingerprint().unwrap(), original.fingerprint().unwrap());
}

#[test]
fn rejects_duplicate_parameter_keys_unknown_fields_and_incomplete_documents() {
    let valid = playback(500).encode().unwrap();
    let duplicate = valid.replace(
        "\"colour\":\"red\"",
        "\"colour\":\"red\",\"colour\":\"blue\"",
    );
    assert!(
        Playback::decode(duplicate.as_bytes()).is_err(),
        "duplicate nested keys must not silently win"
    );
    let contradictory = valid.replacen('{', "{\"power\":false,", 1);
    assert!(
        Playback::decode(contradictory.as_bytes()).is_err(),
        "independent power flag is not a second authority"
    );
    let missing = valid.replace("\"effect\":null,", "");
    assert!(Playback::decode(missing.as_bytes()).is_err());
}

#[test]
fn validates_schema_parameter_limits_and_wire_size_on_both_paths() {
    let mut value = playback(1000);
    value.schema = 2;
    assert!(value.encode().is_err());
    let wrong_schema = playback(1000)
        .encode()
        .unwrap()
        .replace("\"schema\":1", "\"schema\":2");
    assert!(Playback::decode(wrong_schema.as_bytes()).is_err());
    value.schema = 1;
    value.parameters.insert("wide".into(), "x".repeat(1025));
    assert!(value.encode().is_err());
    value.parameters.clear();
    for index in 0..40 {
        value
            .parameters
            .insert(format!("key{index}"), "x".repeat(1000));
    }
    assert!(value.encode().is_err());
    assert!(Playback::decode(&vec![b' '; MAX_PLAYBACK_BYTES + 1]).is_err());
}

#[test]
fn operation_order_prevents_replay_from_undoing_stop() {
    let stopped = OperationStamp {
        revision: Revision(11),
        fingerprint: "stop".into(),
    };
    let old = OperationStamp {
        revision: Revision(10),
        fingerprint: "playback".into(),
    };
    assert!(classify_operation(Some(&stopped), &old).is_err());
    assert_eq!(
        classify_operation(Some(&stopped), &stopped).unwrap(),
        Admission::Duplicate
    );
    let conflict = OperationStamp {
        fingerprint: "restart".into(),
        ..stopped.clone()
    };
    assert!(classify_operation(Some(&stopped), &conflict).is_err());
    let newer = OperationStamp {
        revision: Revision(12),
        ..conflict
    };
    assert_eq!(
        classify_operation(Some(&stopped), &newer).unwrap(),
        Admission::New
    );
}

#[test]
fn brightness_and_independent_planes_keep_hidden_values_but_zero_is_off() {
    let mixed = ChannelPlanes {
        dimmable: 200,
        independent: 25,
    };
    assert_eq!(mixed.render(Level::FULL), 225);
    assert_eq!(mixed.render(Level::try_from(500).unwrap()), 125);
    assert_eq!(mixed.render(Level::OFF), 0);
    assert_eq!(
        mixed.render(Level::FULL),
        225,
        "zero level must not erase underlying intensity"
    );
    let no_dimming = ChannelPlanes {
        dimmable: 0,
        independent: 250,
    };
    assert_eq!(no_dimming.render(Level::try_from(1).unwrap()), 250);
    assert_eq!(
        no_dimming.render(Level::OFF),
        0,
        "zero level is off even for NoDimming output"
    );
    let saturating = ChannelPlanes {
        dimmable: 255,
        independent: 255,
    };
    assert_eq!(saturating.render(Level::FULL), 255);
}
