use dmx_protocol::{
    configuration::{Array, Configuration, Content, Universe},
    Id, Level, Playback, Revision,
};
use mqtt_dmx::{
    compiler::CompiledConfiguration, engine::Status, ordered_array::OrderedArray,
    recovery::Snapshot, recovery_file::FallbackFile,
};
use std::collections::{BTreeMap, BTreeSet};
fn id(n: u8) -> Id {
    format!("00000000-0000-0000-0000-{n:012}")
        .try_into()
        .unwrap()
}
fn snapshot() -> Snapshot {
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
    let configuration = Configuration {
        schema: 1,
        history: id(4),
        authority: id(5),
        revision: Revision(1),
        expected_previous: None,
        digest: content.digest().unwrap(),
        content,
    };
    let compiled = CompiledConfiguration::compile(configuration.clone()).unwrap();
    let playback = Playback {
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
    };
    let effect = compiled
        .effect(&id(2), None, &BTreeMap::new(), false, 200)
        .unwrap();
    let mut array = OrderedArray::new(
        compiled.ownership()[&id(2)].clone(),
        playback,
        effect,
        id(6),
    )
    .unwrap();
    array.tick();
    Snapshot {
        schema: 1,
        history: id(4),
        authority: id(5),
        revision: Revision(2),
        configuration,
        arrays: BTreeMap::from([(id(2), array.checkpoint().unwrap())]),
        retired_ids: BTreeSet::new(),
    }
}

#[test]
fn installation_roundtrip_validates_every_array_before_restoring() {
    let s = snapshot();
    let document = s.encode().unwrap();
    let restored = Snapshot::decode(document.as_bytes()).unwrap();
    assert_eq!(restored.document, document);
    assert_eq!(restored.digest, dmx_protocol::sha256(document.as_bytes()));
    let channel = &restored.configuration.ownership()[&id(2)][0];
    assert_eq!(
        restored.arrays[&id(2)].output(channel),
        0,
        "running effect starts at original output"
    );
    assert_eq!(restored.arrays[&id(2)].receipt().status, Status::Running);
}

#[test]
fn invalid_installation_is_refused_instead_of_partially_restored() {
    for change in 0..6 {
        let mut s = snapshot();
        match change {
            0 => s.history = id(9),
            1 => s.authority = id(9),
            2 => {
                s.arrays.clear();
            }
            3 => {
                s.retired_ids.insert(id(2));
            }
            4 => {
                let a = s.configuration.content.arrays.get_mut(&id(2)).unwrap();
                a.generation = id(9);
                s.configuration.digest = s.configuration.content.digest().unwrap();
            }
            _ => s.arrays.get_mut(&id(2)).unwrap().player.held.clear(),
        };
        assert!(s.encode().is_err());
        let raw = serde_json::to_vec(&s).unwrap();
        assert!(Snapshot::decode(&raw).is_err());
    }
}

#[test]
fn corrupted_unknown_duplicate_and_oversized_recovery_documents_are_refused() {
    let s = snapshot();
    let document = s.encode().unwrap();
    let duplicate = document.replacen("\"schema\":1", "\"schema\":1,\"schema\":1", 1);
    assert!(Snapshot::decode(duplicate.as_bytes()).is_err());
    let unknown = document.replacen('{', "{\"unexpected\":0,", 1);
    assert!(Snapshot::decode(unknown.as_bytes()).is_err());
    assert!(Snapshot::decode(&vec![b' '; 256 * 1024 + 1]).is_err());
    let mut s = snapshot();
    s.schema = 9;
    assert!(s.encode().is_err());
}

#[test]
fn explicitly_saved_validated_recovery_roundtrips_without_writes_during_ticks() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fallback.json");
    let mut file = FallbackFile::new(path.clone());
    let document = snapshot().encode().unwrap();
    let mut restored = Snapshot::decode(document.as_bytes()).unwrap();
    for _ in 0..8 {
        restored.arrays.get_mut(&id(2)).unwrap().tick();
    }
    assert!(!path.exists());
    file.save(id(8), &document).unwrap();
    let saved = file.load().unwrap().unwrap();
    assert_eq!(saved.snapshot, document);
    let restored = Snapshot::decode(saved.snapshot.as_bytes()).unwrap();
    assert_eq!(restored.arrays[&id(2)].receipt().status, Status::Running);
}

#[test]
fn running_recovery_has_one_fixed_representation_between_frames() {
    let mut s = snapshot();
    s.arrays.get_mut(&id(2)).unwrap().player.held[0]
        .planes
        .dimmable = 19;
    assert!(
        s.encode().is_err(),
        "running held planes must be the original initial planes"
    );
}
