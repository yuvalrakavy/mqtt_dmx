use dmx_protocol::{
    configuration::{Array, Configuration, Content, Universe},
    Id, Level, Playback, Revision,
};
use mqtt_dmx::{
    installation::Installation,
    service_v2::{bridge, DmxService},
};
use mqtt_test_broker::{FakeBroker, SendOptions};
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    time::{Duration, Instant},
};

fn id(value: u8) -> Id {
    format!("00000000-0000-0000-0000-{value:012}")
        .try_into()
        .unwrap()
}

#[test]
fn v2_cli_names_the_binary_and_documents_the_actual_default_root() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_mqtt_dmx2"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("Usage: mqtt_dmx2"));
    assert!(
        help.contains("MQTT root (default: DMX)"),
        "help advertised the wrong broker root: {help}"
    );
    assert!(help.contains("--instance"));
    assert!(help.contains("--startup-fallback"));
}

#[tokio::test]
async fn legacy_protocol_on_a_separate_broker_cannot_change_default_root_v2_output() {
    // Exercise the v1 wire contract on its own broker. The preserved v1 binary
    // takes a host on fixed port 1883, so this probe uses its MQTT identity and
    // topics with an ephemeral port rather than touching an installed broker.
    let legacy = FakeBroker::start_multi().await;
    let current = FakeBroker::start().await;
    let output = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    current.retain(
        "DMX/State/one/_recovery",
        serde_json::to_vec(&serde_json::json!({"snapshot":snapshot(output.local_addr().unwrap())}))
            .unwrap(),
        &SendOptions::default(),
    );
    let directory = tempfile::tempdir().unwrap();
    let running = bridge()
        .instance("one")
        .broker(&current.address())
        .start_managed(DmxService::new(directory.path().join("v2-startup.json")))
        .await
        .unwrap();
    let options = rumqttc::v5::MqttOptions::new("DMX", "127.0.0.1", legacy.port());
    let (client, mut eventloop) = rumqttc::v5::AsyncClient::new(options, 10);
    let pump = tokio::spawn(async move {
        loop {
            eventloop.poll().await.unwrap();
        }
    });
    use rumqttc::v5::mqttbytes::QoS;
    client.subscribe("DMX/#", QoS::AtLeastOnce).await.unwrap();
    for (topic, payload) in [("DMX/Active", "true"), ("DMX/Universe/one", "{\"description\":\"legacy\",\"controller\":\"127.0.0.1\",\"net\":0,\"subnet\":0,\"universe\":0,\"channels\":2,\"log\":false,\"disable_send\":true}"), ("DMX/Command/Set", "{\"universe_id\":\"one\",\"channels\":\"s:1\",\"target\":\"s(0)\",\"dimming_amount\":1000}")] {
        client.publish(topic, QoS::AtLeastOnce, true, payload).await.unwrap();
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while !legacy
            .received()
            .iter()
            .any(|p| p.topic == "DMX/Command/Set")
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let mut packet = [0u8; 1024];
        loop {
            let (length, source) = output.recv_from(&mut packet).await.unwrap();
            assert!(source.ip().is_loopback());
            assert_eq!(&packet[..8], b"Art-Net\0");
            if length > 19 && packet[19] == 255 {
                break;
            }
        }
        for _ in 0..3 {
            output.recv_from(&mut packet).await.unwrap();
            assert_eq!(packet[19], 255, "legacy broker changed v2 lighting");
        }
    })
    .await
    .expect("separate-broker coexistence did not produce both legacy traffic and v2 output");
    assert!(current
        .received()
        .iter()
        .all(|p| p.topic != "DMX/Command/Set" && p.topic != "DMX/Universe/one"));
    assert!(legacy
        .received()
        .iter()
        .all(|p| !p.topic.starts_with("DMX/State/") && !p.topic.starts_with("DMX/Status/")));
    assert!(!directory.path().join("v2-startup.json").exists());
    assert!(running.stop().await);
    pump.abort();
    let _ = pump.await;
    for binary in [
        env!("CARGO_BIN_EXE_mqtt_dmx"),
        env!("CARGO_BIN_EXE_mqtt_dmx2"),
    ] {
        let help = std::process::Command::new(binary)
            .arg("--help")
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&help.stdout),
            String::from_utf8_lossy(&help.stderr)
        );
        assert!(text.contains("Usage: mqtt_dmx"), "{binary}: {text}");
    }
}
fn snapshot(endpoint: SocketAddr) -> String {
    let mut content = Content::default();
    content.universes.insert(
        id(1),
        Universe {
            name: "main".into(),
            description: String::new(),
            endpoint,
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
    let mut live = Installation::initialize(configuration, [(id(2), id(6))].into()).unwrap();
    live.desired(
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
    live.document().to_string()
}

#[tokio::test]
async fn published_definitions_get_bridge_owned_generations_without_restarting_playback() {
    let output = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let document = snapshot(output.local_addr().unwrap());
    let original = Installation::restore(document.as_bytes()).unwrap();
    let broker = FakeBroker::start().await;
    broker.retain(
        "DMX/State/one/_recovery",
        serde_json::to_vec(&serde_json::json!({"snapshot":document})).unwrap(),
        &SendOptions::default(),
    );
    let directory = tempfile::tempdir().unwrap();
    let running = bridge()
        .instance("one")
        .broker(&broker.address())
        .start_managed(DmxService::new(directory.path().join("startup.json")))
        .await
        .unwrap();
    assert!(
        broker
            .wait_for_subscription("DMX/Desired/one/+/+", Duration::from_secs(3))
            .await
    );
    let mut publication = original.configuration().clone();
    publication.revision = Revision(2);
    publication.expected_previous = Some(Revision(1));
    let array = publication.content.arrays.get_mut(&id(2)).unwrap();
    array.description = "Published label edit".into();
    // A publisher's placeholder cannot rotate the live array's mapping authority.
    array.generation = id(90);
    publication.digest = publication.content.digest().unwrap();
    assert!(broker.send_with(
        "DMX/Config/one/configuration",
        publication.encode().unwrap().into_bytes(),
        &SendOptions::default()
            .expiry(10)
            .user_property("target-generation", &String::from(id(5)))
    ));
    assert!(
        broker
            .wait_until(Duration::from_secs(3), |b| b
                .retained_on("DMX/State/one/configuration")
                .is_some_and(|bytes| {
                    let state: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                    let configuration =
                        Configuration::decode(state["configuration"].as_str().unwrap().as_bytes())
                            .unwrap();
                    configuration.revision == Revision(2)
                }))
            .await,
        "published definition was not accepted: {:?}",
        broker.received_on("DMX/Error/one")
    );
    let state: serde_json::Value =
        serde_json::from_slice(&broker.retained_on("DMX/State/one/configuration").unwrap())
            .unwrap();
    let accepted =
        Configuration::decode(state["configuration"].as_str().unwrap().as_bytes()).unwrap();
    assert_eq!(accepted.content.arrays[&id(2)].generation, id(3));
    let array_state: serde_json::Value = serde_json::from_slice(
        &broker
            .retained_on(&format!("DMX/State/one/{}", String::from(id(2))))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        array_state["execution"],
        String::from(id(7)),
        "a definition-only publish restarted playback"
    );
    assert!(running.stop().await);
}

async fn reply(
    broker: &FakeBroker,
    body: serde_json::Value,
    generation: &str,
) -> serde_json::Value {
    let topic = format!("Test/reply/{}", uuid::Uuid::new_v4());
    let options = SendOptions::default()
        .expiry(10)
        .response_topic(&topic)
        .user_property("target-generation", generation);
    assert!(broker.send_with(
        "DMX/Command/one",
        serde_json::to_vec(&body).unwrap(),
        &options
    ));
    assert!(
        broker
            .wait_until(Duration::from_secs(3), |broker| !broker
                .received_on(&topic)
                .is_empty())
            .await
    );
    serde_json::from_slice(&broker.received_on(&topic)[0].payload).unwrap()
}

#[tokio::test]
async fn desired_stop_restart_and_explicit_fallback_save_cross_the_real_proxy() {
    let output = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let document = snapshot(output.local_addr().unwrap());
    let broker = FakeBroker::start().await;
    broker.retain(
        "DMX/State/one/_recovery",
        serde_json::to_vec(&serde_json::json!({"snapshot":document})).unwrap(),
        &SendOptions::default(),
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("startup.json");
    let running = bridge()
        .instance("one")
        .broker(&broker.address())
        .start_managed(DmxService::new(path.clone()))
        .await
        .unwrap();
    assert!(
        broker
            .wait_for_subscription("DMX/Desired/one/+/+", Duration::from_secs(3))
            .await
    );
    let mut playback: Playback = serde_json::from_str(
        &serde_json::from_str::<mqtt_dmx::recovery::Snapshot>(&document)
            .unwrap()
            .arrays[&id(2)]
            .playback,
    )
    .unwrap();
    playback.revision = Revision(2);
    playback.level = 500.try_into().unwrap();
    playback.fade_ms = 0;
    let scalar = playback.encode().unwrap();
    assert!(broker.send_with(
        &format!("DMX/Desired/one/{}/playback", String::from(id(2))),
        serde_json::to_vec(&scalar).unwrap(),
        &SendOptions::default()
            .expiry(10)
            .user_property("target-generation", &String::from(id(3)))
    ));
    let state_topic = format!("DMX/State/one/{}", String::from(id(2)));
    assert!(
        broker
            .wait_until(Duration::from_secs(3), |broker| broker
                .retained_on(&state_topic)
                .is_some_and(|bytes| {
                    let state: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                    state["playback"] == scalar
                }))
            .await,
        "Desired never reached the engine"
    );
    let state: serde_json::Value =
        serde_json::from_slice(&broker.retained_on(&state_topic).unwrap()).unwrap();
    let body = serde_json::json!({"target":String::from(id(2)),"command":"Stop","revision":"3","expected_playback":"2",
        "expected_execution":state["execution"],"history":id(4),"authority":id(5),"generation":id(3)});
    let answer = reply(&broker, body.clone(), &String::from(id(3))).await;
    assert!(answer.get("error").is_none(), "Stop was refused: {answer}");
    let mut restart = body;
    restart["command"] = "Restart".into();
    restart["revision"] = "4".into();
    let answer = reply(&broker, restart, &String::from(id(3))).await;
    assert!(
        answer.get("error").is_none(),
        "Restart was refused: {answer}"
    );
    assert!(!path.exists());
    let answer = reply(
        &broker,
        serde_json::json!({"command":"SaveStartupFallback","save_id":id(30)}),
        &String::from(id(5)),
    )
    .await;
    assert!(
        answer.get("error").is_none(),
        "explicit fallback save failed: {answer}"
    );
    let saved = mqtt_dmx::recovery_file::FallbackFile::new(path)
        .load()
        .unwrap()
        .unwrap();
    assert_eq!(saved.save_id, id(30));
    let recovered = Installation::restore(saved.snapshot.as_bytes()).unwrap();
    assert_eq!(recovered.receipt(&id(2)).unwrap().revision, Revision(4));
    assert!(running.stop().await);
}

#[tokio::test]
async fn managed_service_restores_broker_snapshot_and_emits_real_loopback_frames() {
    let output = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let document = snapshot(output.local_addr().unwrap());
    let broker = FakeBroker::start().await;
    broker.publish(
        "DMX/State/one/_recovery",
        serde_json::to_vec(&serde_json::json!({"snapshot":document})).unwrap(),
        &SendOptions::default().retained(),
    );
    let directory = tempfile::tempdir().unwrap();
    let fallback = directory.path().join("startup.json");
    let running = bridge()
        .instance("one")
        .broker(&broker.address())
        .start_managed(DmxService::new(fallback.clone()))
        .await
        .expect("DMX managed service did not start");
    let mut values = Vec::new();
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut packet = [0u8; 1024];
        while values.last() != Some(&255) {
            let (length, source) = output.recv_from(&mut packet).await.unwrap();
            assert!(source.ip().is_loopback());
            assert_eq!(length, 530);
            assert_eq!(&packet[..8], b"Art-Net\0");
            assert_ne!(packet[12], 0);
            values.push(packet[19]);
        }
    })
    .await
    .expect("restored animation never reached loopback output");
    assert!(values.iter().any(|value| *value > 0 && *value < 255));
    assert!(values.windows(2).all(|pair| pair[0] <= pair[1]));
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(bytes) = broker.retained_on("DMX/State/one/_recovery") {
                let carrier: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                let saved: mqtt_dmx::recovery::Snapshot =
                    serde_json::from_str(carrier["snapshot"].as_str().unwrap()).unwrap();
                if saved.arrays[&id(2)].player.status == mqtt_dmx::engine::Status::Completed {
                    assert!(saved.revision > Revision(1));
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("completed output was never replicated to the broker");
    broker.drop_connection();
    tokio::time::timeout(Duration::from_secs(15), async {
        let mut packet = [0u8; 1024];
        let mut reconnected_frames = 0;
        while reconnected_frames < 5 {
            output.recv_from(&mut packet).await.unwrap();
            assert_eq!(packet[19], 255, "reconnect restarted the completed effect");
            if broker.accepted() >= 2 {
                reconnected_frames += 1;
            }
        }
    })
    .await
    .expect("service did not reconnect while continuing output");
    assert!(
        !fallback.exists(),
        "ordinary snapshot recovery wrote the startup fallback"
    );
    assert!(running.stop().await);
}

async fn assert_bootstrap_setup(corrupt: bool) {
    let broker = FakeBroker::start().await;
    if corrupt {
        broker.retain(
            "DMX/State/one/_recovery",
            "invalid snapshot",
            &SendOptions::default(),
        );
    }
    let directory = tempfile::tempdir().unwrap();
    let running = bridge()
        .instance("one")
        .broker(&broker.address())
        .start_managed(DmxService::new(directory.path().join("startup.json")))
        .await
        .unwrap();
    assert!(
        broker
            .wait_for_subscription("DMX/Command/one", Duration::from_secs(3))
            .await
    );
    let topic = "Test/discover";
    assert!(broker.send_with(
        "DMX/Command/one",
        br#"{"command":"ReadRecovery"}"#.as_slice(),
        &SendOptions::default().expiry(10).response_topic(topic)
    ));
    assert!(
        broker
            .wait_until(Duration::from_secs(3), |broker| !broker
                .received_on(topic)
                .is_empty())
            .await
    );
    let answer: serde_json::Value =
        serde_json::from_slice(&broker.received_on(topic)[0].payload).unwrap();
    assert!(
        answer.get("error").is_none(),
        "read-only bootstrap discovery was refused: {answer}"
    );
    let generation = answer["value"]["generation"].as_str().unwrap();
    assert!(
        answer["value"].get("snapshot").is_none(),
        "discovery returned a Store backup"
    );
    let output = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let original: mqtt_dmx::recovery::Snapshot =
        serde_json::from_str(&snapshot(output.local_addr().unwrap())).unwrap();
    let setup = serde_json::json!({"command":"Initialize","configuration":original.configuration});
    let refused_topic = "Test/initialize-unfenced";
    assert!(broker.send_with(
        "DMX/Command/one",
        serde_json::to_vec(&setup).unwrap(),
        &SendOptions::default()
            .expiry(10)
            .response_topic(refused_topic)
    ));
    assert!(
        broker
            .wait_until(Duration::from_secs(3), |broker| !broker
                .received_on(refused_topic)
                .is_empty())
            .await
    );
    let refused: serde_json::Value =
        serde_json::from_slice(&broker.received_on(refused_topic)[0].payload).unwrap();
    assert!(
        refused["error"].is_string(),
        "mutation inherited the discovery exception"
    );
    let initialized = reply(&broker, setup, generation).await;
    assert!(
        initialized.get("error").is_none(),
        "default initialization failed: {initialized}"
    );
    let mut packet = [0u8; 1024];
    let (length, _) = tokio::time::timeout(Duration::from_secs(3), output.recv_from(&mut packet))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(length, 530);
    assert!(packet[18..length].iter().all(|value| *value == 0));
    assert!(
        broker
            .wait_until(Duration::from_secs(3), |broker| broker
                .retained_on("DMX/State/one/_recovery")
                .is_some())
            .await
    );
    let retained: serde_json::Value =
        serde_json::from_slice(&broker.retained_on("DMX/State/one/_recovery").unwrap()).unwrap();
    let restored =
        Installation::restore(retained["snapshot"].as_str().unwrap().as_bytes()).unwrap();
    assert_eq!(
        restored.receipt(&id(2)).unwrap().status,
        mqtt_dmx::engine::Status::Completed
    );
    assert_ne!(restored.configuration().history, id(4));
    assert_ne!(restored.configuration().authority, id(5));
    assert_ne!(
        restored.configuration().content.arrays[&id(2)].generation,
        id(3)
    );
    assert!(restored.states().unwrap()[&id(2)]
        .playback
        .contains("\"level\":0"));
    assert!(running.stop().await);
}

#[tokio::test]
async fn empty_bridge_can_be_discovered_and_initialized_off_without_store_backup() {
    assert_bootstrap_setup(false).await;
}

#[tokio::test]
async fn unusable_broker_snapshot_allows_explicit_default_off_setup_without_store_backup() {
    assert_bootstrap_setup(true).await;
}

#[tokio::test]
async fn saved_fallback_runs_without_mqtt_and_completed_output_survives_reconnection() {
    let output = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let document = snapshot(output.local_addr().unwrap());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("startup.json");
    mqtt_dmx::recovery_file::FallbackFile::new(path.clone())
        .save(id(30), &document)
        .unwrap();
    let original_file = std::fs::read(&path).unwrap();
    let broker = FakeBroker::start().await;
    broker.retain(
        "DMX/State/one/_recovery",
        serde_json::to_vec(&serde_json::json!({"snapshot":document})).unwrap(),
        &SendOptions::default(),
    );
    broker.refuse_connections(true);
    let running = bridge()
        .instance("one")
        .broker(&broker.address())
        .mqtt_timing(mqtt_bridge_kit::MqttTiming {
            retry: Duration::from_millis(200),
            poll_retry: Duration::from_millis(100),
            ..Default::default()
        })
        .start_managed(DmxService::new(path.clone()))
        .await
        .unwrap();
    let mut packet = [0u8; 1024];
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            output.recv_from(&mut packet).await.unwrap();
            if packet[19] == 255 {
                break;
            }
        }
    })
    .await
    .expect("fallback did not finish while MQTT was unavailable");
    broker.refuse_connections(false);
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            output.recv_from(&mut packet).await.unwrap();
            assert_eq!(
                packet[19], 255,
                "replay restarted a locally completed fallback effect"
            );
            if let Some(bytes) = broker.retained_on("DMX/State/one/_recovery") {
                let carrier: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                let snapshot: mqtt_dmx::recovery::Snapshot =
                    serde_json::from_str(carrier["snapshot"].as_str().unwrap()).unwrap();
                if snapshot.arrays[&id(2)].player.status == mqtt_dmx::engine::Status::Completed {
                    break;
                }
            }
        }
    })
    .await
    .expect("completed fallback was not promoted after reconnect");
    assert_eq!(
        std::fs::read(path).unwrap(),
        original_file,
        "routine recovery rewrote the explicit fallback"
    );
    assert!(running.stop().await);
}
