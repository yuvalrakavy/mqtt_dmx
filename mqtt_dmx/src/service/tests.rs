//! The MQTT session under saturation (Store no-hang §14.3, mqtt_dmx): the task that polls
//! rumqttc's event loop must never wait on rumqttc's request channel, which only polling drains —
//! not directly, not through the error queue to the publisher, and not through ArtNet work that
//! waits on that queue. Each test drives the whole bridge against an in-process fake broker that
//! withholds its acknowledgements, so the request channel fills.

use std::path::PathBuf;
use std::time::Duration;

use mqtt_test_broker::FakeBroker;

use super::{broker_host_port, Service, ServiceConfig, Started};
use crate::persistence::{read_config, Persistence};

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

/// The bridge says it is active only once it has subscribed (no-hang F4): an `Active=true` before
/// the subscription is a bridge the Store takes as ready while its commands go nowhere. Then, after
/// `Active`, every connection republishes the retained model from the bridge's state — its version
/// and its last error report — for a broker that lost its retained messages (the fleet's order:
/// subscribe, `Active`, the model; Store 3b review round 3, X4). The broker holds its acks with room
/// for one publish in flight, so it sees exactly what the bridge sends first, and nothing after it
/// until the acks are released.
#[tokio::test(flavor = "multi_thread")]
async fn active_goes_out_only_after_the_subscription() {
    const REPORT: &str = r#"{"time":"then","message":"an earlier session's error"}"#;
    let broker = FakeBroker::start_with_receive_max(1).await;
    broker.hold_acks();
    let (client, events) = Service::mqtt_client(&broker.address());
    let (_pump, _incoming) = crate::mqtt_pump::Pump::start(events, std::sync::Arc::new(crate::mqtt_outage::Outage::new()));
    let address = broker.address();
    let last_error = crate::mqtt_publisher::LastError::default();
    last_error.set(REPORT.as_bytes().to_vec());
    let announce = tokio::spawn(async move { Service::announce(&client, &address, &last_error).await });
    let test = async {
        assert!(
            broker.wait_until(Duration::from_secs(10), |b| !b.received().is_empty()).await,
            "the bridge published nothing"
        );
        assert!(
            broker.subscriptions().iter().any(|f| f == "DMX/#"),
            "the bridge published {:?} before it subscribed",
            broker.received().iter().map(|r| r.topic.clone()).collect::<Vec<_>>()
        );
        let first = broker.received()[0].topic.clone();
        assert_eq!(first, "DMX/Active", "the first publish after the subscription was {first}, not Active=true");
        broker.release_acks();
        assert!(
            matches!(tokio::time::timeout(Duration::from_secs(10), announce).await, Ok(Ok(Ok(())))),
            "the announcement did not complete once the broker acknowledged it"
        );
        // Queued is not delivered: the pump sends the rest as the acks come back.
        assert!(
            broker.wait_until(Duration::from_secs(10), |b| !b.received_on("DMX/LastError").is_empty()).await,
            "the model never arrived"
        );
        let topics: Vec<_> = broker.received().iter().map(|r| r.topic.clone()).collect();
        assert_eq!(topics, ["DMX/Active", "DMX/Version", "DMX/LastError"], "not subscribe, Active, then the model");
        let republished = broker.received_on("DMX/LastError");
        assert!(
            republished.len() == 1 && republished[0].retain && republished[0].payload == REPORT.as_bytes(),
            "the connection did not republish the last error report, retained, from the bridge's state: {republished:?}"
        );
    };
    tokio::time::timeout(Duration::from_secs(30), test).await.expect("the test itself ran out of time");
}

#[test]
fn a_broker_address_may_carry_its_port() {
    assert_eq!(broker_host_port("control-tlv"), ("control-tlv", 1883));
    assert_eq!(broker_host_port("127.0.0.1:41883"), ("127.0.0.1", 41883));
    assert_eq!(broker_host_port("broker.local:x"), ("broker.local:x", 1883));
}

/// Stopping is bounded: a task that will not end — inside a synchronous call, where an abort cannot
/// reach it — costs a WARN (`shutdown_timeout`), never a stop that waits on it (Store no-hang 3b
/// review, C-7: SIGTERM now runs this stop, and systemd waits on it). This is the service's stop
/// alone: the test's own runtime is shut down in the background, which proves nothing about the
/// process. That the process then ends within a bound, with a thread stuck, is
/// `tests/process.rs`'s `a_stalled_config_write_does_not_hold_up_the_stop` (re-review X1).
#[test]
fn stopping_is_bounded_when_a_task_will_not_end() {
    const BOUND: Duration = Duration::from_millis(300);
    const STUCK: Duration = Duration::from_secs(3);
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let mut took = Duration::ZERO;
    let events = crate::test_log::capture(|| {
        took = runtime.block_on(async {
            let mut workers = tokio::task::JoinSet::new();
            let (entered, inside) = tokio::sync::oneshot::channel();
            workers.spawn(async move {
                let _ = entered.send(());
                std::thread::sleep(STUCK);
            });
            inside.await.expect("the stuck task started");
            let service = Service::<Started> {
                config: ServiceConfig { mqtt_broker_address: String::new(), storage_path: storage("stop") },
                cancel: Some(tokio_util::sync::CancellationToken::new()),
                workers,
                persistence: None,
                writer: None,
                _status: std::marker::PhantomData,
            };
            let started = std::time::Instant::now();
            let _ = service.stop_within(BOUND).await;
            started.elapsed()
        });
    });
    runtime.shutdown_background();
    assert!(took < STUCK / 2, "stopping waited {took:?} for a task stuck in a synchronous call");
    assert!(
        events.iter().any(|e| e.level == tracing::Level::WARN && e.kind() == Some("shutdown_timeout")),
        "stopping ran past its bound without a WARN: {events:#?}"
    );
}

/// A save is posted to the writer, which writes it later; a stop right after must still let it
/// write what was saved, within the stop's bound, before it is aborted. The test's runtime runs
/// one task at a time, so the writer has not run when the stop begins.
#[tokio::test]
async fn a_stop_writes_what_was_saved_before_it() {
    let storage_path = storage("flush");
    let _ = std::fs::remove_dir_all(&storage_path);
    std::fs::create_dir_all(&storage_path).expect("create the storage directory");
    let persistence = std::sync::Arc::new(Persistence::new(storage_path.clone()));
    let writer = tokio::spawn(Persistence::write_saves(persistence.clone()));
    let mut values = std::collections::HashMap::new();
    values.insert(std::sync::Arc::<str>::from("Level"), "42".to_string());
    persistence.save_values(&values);

    let service = Service::<Started> {
        config: ServiceConfig { mqtt_broker_address: String::new(), storage_path: storage_path.clone() },
        cancel: Some(tokio_util::sync::CancellationToken::new()),
        workers: tokio::task::JoinSet::new(),
        persistence: Some(persistence),
        writer: Some(writer),
        _status: std::marker::PhantomData,
    };
    let _ = tokio::time::timeout(Duration::from_secs(10), service.stop_within(Duration::from_secs(5))).await;
    let saved = read_config(&storage_path).values;
    let _ = std::fs::remove_dir_all(&storage_path);
    assert_eq!(saved.get("Level").map(String::as_str), Some("42"), "the stop dropped a save made before it");
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
/// tick, an error report from the ArtNet manager — while the command that started it succeeds.
const GHOST_ARRAY: &str = r#"{
    "universe_id": "Ghost",
    "description": "An array on a universe nobody defined",
    "lights": { "all": "s:0" },
    "effects": { "on": { "type": "fade", "lights": "@all", "ticks": 4, "target": "s(255)" } }
}"#;

/// A universe on loopback that never sends. Adding one is a command the ArtNet manager must answer
/// and the subscriber then persists — no error report anywhere, so it never touches the error
/// queue, the publisher or the broker, and its completion is seen in the bridge's storage.
const PROBE_UNIVERSE: &str = r#"{
    "description": "A probe", "controller": "127.0.0.1", "net": 0, "subnet": 0, "universe": 0,
    "channels": 8, "disable_send": true
}"#;

/// Waits until `ready()` holds, or `within` passes. Returns whether it held.
async fn eventually(within: Duration, ready: impl Fn() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    while !ready() {
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    true
}

fn persisted_universe(tag: &str, id: &str) -> bool {
    read_config(&storage(tag)).universes.contains_key(id)
}

/// The ArtNet manager's error reports back up behind a stalled publisher — the broker withholds its
/// acks — while the bridge must go on taking commands the ArtNet manager answers. Every command here
/// succeeds at the subscriber, so none of them waits on the error queue itself: what is measured is
/// the ArtNet manager. One that waits on the error queue once it is full stops answering, and the
/// subscriber, waiting on its reply, takes no command until the broker recovers.
#[tokio::test(flavor = "multi_thread")]
async fn commands_that_wait_on_artnet_complete_while_its_errors_back_up_behind_a_stalled_broker() {
    // Each failed effect is one report, two QoS 1 publishes: two in flight, ten in rumqttc's request
    // channel, one in the publisher's hands and ten in the error queue are 18 reports, so 30 fill
    // everything with room to spare.
    const FAILURES: usize = 30;
    let broker = FakeBroker::start_with_receive_max(2).await;
    let service = start_bridge(&broker, "artnet").await;
    let test = async {
        assert!(broker.send("DMX/Array/Ghost", GHOST_ARRAY));
        assert!(
            eventually(Duration::from_secs(5), || read_config(&storage("artnet")).arrays.contains_key("Ghost")).await,
            "the bridge never took the array"
        );
        broker.hold_acks();
        for _ in 0..FAILURES {
            assert!(broker.send("DMX/Command/On", r#"{"array_id": "Ghost"}"#));
            // One failed tick per command: the ArtNet manager ticks every 50 ms.
            tokio::time::sleep(Duration::from_millis(80)).await;
        }
        assert!(broker.held_acks() >= 2, "the broker never stalled: it holds {} acks", broker.held_acks());

        // Still stalled: a command the ArtNet manager must answer.
        assert!(broker.send("DMX/Universe/Probe1", PROBE_UNIVERSE));
        assert!(
            eventually(Duration::from_secs(5), || persisted_universe("artnet", "Probe1")).await,
            "the bridge stopped taking commands while the broker was stalled — the ArtNet manager waited on the full \
             error queue to the stalled publisher, and the subscriber on the ArtNet manager"
        );

        // Recovered: it still takes them.
        broker.release_acks();
        assert!(broker.send("DMX/Universe/Probe2", PROBE_UNIVERSE));
        assert!(
            eventually(Duration::from_secs(10), || persisted_universe("artnet", "Probe2")).await,
            "the bridge took no command after the broker recovered"
        );
        let reported = broker
            .wait_until(Duration::from_secs(10), |b| {
                b.received_on("DMX/Error").iter().any(|r| String::from_utf8_lossy(&r.payload).contains("No universe with ID 'Ghost'"))
            })
            .await;
        assert!(
            reported,
            "no ArtNet error report arrived, so the ArtNet manager's error path was not exercised: {:?}",
            error_reports(&broker)
        );
    };
    bounded(service, "artnet", test).await;
}

/// The events at `level` with `kind`.
fn of_kind<'a>(events: &'a [crate::test_log::Captured], level: tracing::Level, kind: &str) -> Vec<&'a crate::test_log::Captured> {
    events.iter().filter(|e| e.level == level && e.kind() == Some(kind)).collect()
}

/// The events at ERROR, or with `external_failure`: neither belongs to a command that failed.
fn misfiled(events: &[crate::test_log::Captured]) -> Vec<&crate::test_log::Captured> {
    events.iter().filter(|e| e.level == tracing::Level::ERROR || e.kind() == Some("external_failure")).collect()
}

/// A failed command is logged once, where it failed, at the level and with the kind its cause calls
/// for (the logging policy; Store no-hang 3b, round 2): a command the bridge cannot parse is input
/// that failed validation, INFO `validation_rejected`; one the ArtNet or array manager refuses — an
/// array nobody defined, an effect that fails at its first tick — is WARN `command_rejected`: the
/// Store's configuration needs fixing. Never an ERROR, since no code change is warranted, and never
/// `external_failure`, since nothing external is down. The old bridge logged each twice: an ERROR
/// `decode_error` where it failed, and a WARN `external_failure` where its report was published.
/// One runtime thread, so every task logs where the capture is.
#[test]
fn a_failed_command_is_logged_once_with_the_kind_its_cause_calls_for() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let events = crate::test_log::capture(|| {
        runtime.block_on(async {
            let broker = FakeBroker::start().await;
            let service = start_bridge(&broker, "kinds").await;
            let test = async {
                // Malformed: not a DMX subtopic, and a payload that is not JSON.
                assert!(broker.send("DMX/Bogus", "{}"));
                assert!(broker.send("DMX/Command/On", "not json"));
                // Refused: an array nobody defined.
                assert!(broker.send("DMX/Command/On", r#"{"array_id": "Nobody"}"#));
                // Accepted, then failing at its first tick: an array on a universe nobody defined.
                assert!(broker.send("DMX/Array/Ghost", GHOST_ARRAY));
                assert!(broker.send("DMX/Command/On", r#"{"array_id": "Ghost"}"#));
                assert!(
                    broker.wait_until(Duration::from_secs(10), |b| b.received_on("DMX/Error").len() >= 4).await,
                    "not every failure was reported: {:?}",
                    error_reports(&broker)
                );
            };
            bounded(service, "kinds", test).await;
        });
    });
    assert!(misfiled(&events).is_empty(), "a failed command was logged as an ERROR or an external_failure: {:#?}", misfiled(&events));
    let rejected = of_kind(&events, tracing::Level::INFO, "validation_rejected");
    assert_eq!(rejected.len(), 2, "the malformed commands were not each one INFO validation_rejected: {events:#?}");
    let refused = of_kind(&events, tracing::Level::WARN, "command_rejected");
    assert_eq!(refused.len(), 2, "the refused commands were not each one WARN command_rejected: {events:#?}");
}

/// A saved configuration the managers refuse when it is restored at startup — a universe number
/// past 15 — is a command refused, WARN `command_rejected`, not an ERROR `decode_error`: the file
/// was read and parsed; what it says needs fixing, not the code.
#[test]
fn a_saved_config_the_managers_refuse_on_restore_is_a_warn() {
    let storage_path = storage("restore");
    let _ = std::fs::remove_dir_all(&storage_path);
    std::fs::create_dir_all(&storage_path).expect("create the storage directory");
    std::fs::write(
        storage_path.join("universes.json"),
        r#"{"Bad": {"description": "past 15", "controller": "127.0.0.1", "net": 0, "subnet": 0, "universe": 99,
            "channels": 8, "disable_send": true}}"#,
    )
    .expect("write the saved universes");
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let events = crate::test_log::capture(|| {
        runtime.block_on(async {
            let broker = FakeBroker::start().await;
            let config = ServiceConfig { mqtt_broker_address: broker.address(), storage_path: storage_path.clone() };
            let service = Service::new(config).start().await;
            let _ = tokio::time::timeout(Duration::from_secs(5), service.stop()).await;
        });
    });
    let _ = std::fs::remove_dir_all(&storage_path);
    assert!(misfiled(&events).is_empty(), "a refused restore was logged as an ERROR or an external_failure: {:#?}", misfiled(&events));
    let refused = of_kind(&events, tracing::Level::WARN, "command_rejected");
    assert!(
        refused.len() == 1 && refused[0].field("universe_id") == Some("Bad"),
        "the refused restore was not one WARN command_rejected naming the universe: {events:#?}"
    );
}
