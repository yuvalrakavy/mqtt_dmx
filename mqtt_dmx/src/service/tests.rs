//! The MQTT session under saturation (Store no-hang §14.3, mqtt_dmx): the task that polls
//! rumqttc's event loop must never wait on rumqttc's request channel, which only polling drains —
//! not directly, not through the error queue to the publisher, and not through ArtNet work that
//! waits on that queue. Each test drives the whole bridge against an in-process fake broker that
//! withholds its acknowledgements, so the request channel fills.

use std::path::PathBuf;
use std::time::Duration;

use mqtt_test_broker::FakeBroker;

use super::{broker_host_port, Service, ServiceConfig, Started};

fn storage(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("mqtt-dmx-{tag}-{}", std::process::id()))
}

/// The whole bridge against `broker`, with empty storage, once it has subscribed.
async fn start_bridge(broker: &FakeBroker, tag: &str) -> Service<Started> {
    let storage_path = storage(tag);
    let _ = std::fs::remove_dir_all(&storage_path);
    let config = ServiceConfig { mqtt_broker_address: broker.address(), storage_path };
    let service = Service::new(config).start().await;
    assert!(broker.wait_for_subscription("DMX/#", Duration::from_secs(10)).await, "the bridge never subscribed");
    service
}

/// Runs `test` bounded, then stops the bridge (bounded too) and removes its storage. A deadlocked
/// bridge is dropped with the test, never waited on.
async fn bounded(service: Service<Started>, tag: &str, test: impl std::future::Future<Output = ()>) {
    let result = tokio::time::timeout(Duration::from_secs(60), test).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), service.stop()).await;
    let _ = std::fs::remove_dir_all(storage(tag));
    result.expect("the test itself ran out of time");
}

fn error_reports(broker: &FakeBroker) -> Vec<String> {
    broker.received_on("DMX/Error").iter().map(|r| String::from_utf8_lossy(&r.payload).into_owned()).collect()
}

#[test]
fn a_broker_address_may_carry_its_port() {
    assert_eq!(broker_host_port("control-tlv"), ("control-tlv", 1883));
    assert_eq!(broker_host_port("127.0.0.1:41883"), ("127.0.0.1", 41883));
    assert_eq!(broker_host_port("broker.local:x"), ("broker.local:x", 1883));
}

/// A burst of commands the bridge cannot parse, while the broker withholds its acknowledgements:
/// each is an error report, two QoS 1 publishes, so the request channel fills, then the error
/// queue behind it. The acks are then released, and every report must arrive. A poller that sends
/// its own error reports waits on that queue, never polls again, never reads the acks — and
/// nothing more arrives.
#[tokio::test(flavor = "multi_thread")]
async fn a_burst_of_failing_commands_against_a_stalled_broker_reports_every_error_once_it_recovers() {
    const COMMANDS: usize = 100;
    let broker = FakeBroker::start_with_receive_max(2).await;
    let service = start_bridge(&broker, "burst").await;
    let test = async {
        broker.hold_acks();
        for i in 0..COMMANDS {
            // Not a DMX subtopic.
            assert!(broker.send(&format!("DMX/Bogus{i}"), "{}"));
        }
        // Let the burst saturate: the request channel, the error queue, and a poller that waits on them.
        tokio::time::sleep(Duration::from_secs(2)).await;
        broker.release_acks();
        let done = broker.wait_until(Duration::from_secs(20), |b| b.received_on("DMX/Error").len() >= COMMANDS).await;
        assert!(
            done,
            "{} of {COMMANDS} error reports arrived after the broker recovered — the poller waited on the error queue, \
             which waits on the request channel only the poller drains",
            error_reports(&broker).len()
        );
    };
    bounded(service, "burst", test).await;
}

/// An array on a universe that does not exist: every effect started on it fails its first ArtNet
/// tick, an error report from the ArtNet manager.
const GHOST_ARRAY: &str = r#"{
    "universe_id": "Ghost",
    "description": "An array on a universe nobody defined",
    "lights": { "all": "s:0" },
    "effects": { "on": { "type": "fade", "lights": "@all", "ticks": 4, "target": "s(255)" } }
}"#;

/// Valid commands that wait on the ArtNet manager, while the broker withholds its acks and the
/// ArtNet manager's own error reports back up behind the stalled publisher. Once the broker
/// recovers the bridge must take commands again. A poller that waits on the ArtNet manager's reply,
/// while the ArtNet manager waits on the error queue, never polls again.
#[tokio::test(flavor = "multi_thread")]
async fn commands_that_wait_on_artnet_while_its_errors_back_up_complete_once_the_broker_recovers() {
    const EFFECTS: usize = 30;
    let broker = FakeBroker::start_with_receive_max(2).await;
    let service = start_bridge(&broker, "artnet").await;
    let test = async {
        assert!(broker.send("DMX/Array/Ghost", GHOST_ARRAY));
        broker.hold_acks();
        for _ in 0..EFFECTS {
            assert!(broker.send("DMX/Command/On", r#"{"array_id": "Ghost"}"#));
            // One failed tick per command: the ArtNet manager ticks every 50 ms.
            tokio::time::sleep(Duration::from_millis(80)).await;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        broker.release_acks();
        // A command whose own error report shows the bridge takes commands again.
        assert!(broker.send("DMX/Sentinel", "{}"));
        let done = broker
            .wait_until(Duration::from_secs(20), |b| {
                b.received_on("DMX/Error").iter().any(|r| String::from_utf8_lossy(&r.payload).contains("Sentinel"))
            })
            .await;
        assert!(
            done,
            "the bridge took no command after the broker recovered ({} error reports arrived) — the poller waited on \
             ArtNet work, which waited on the error queue to the stalled publisher",
            error_reports(&broker).len()
        );
        assert!(
            error_reports(&broker).iter().any(|r| r.contains("No universe with ID 'Ghost'")),
            "no ArtNet error report arrived, so the ArtNet manager's error path was not exercised: {:?}",
            error_reports(&broker)
        );
    };
    bounded(service, "artnet", test).await;
}
