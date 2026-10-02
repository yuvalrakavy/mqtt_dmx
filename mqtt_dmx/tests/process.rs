//! The bridge as a process: systemd's stop (SIGTERM) runs its bounded shutdown and exits cleanly,
//! and a log destination that cannot start costs that destination alone (Store no-hang 3b review,
//! C-7 and C-8); a stalled disk holds up neither its commands nor its stop (re-review X1, F1).
//!
//! Each test runs the built binary as a child, against an in-process fake broker on 127.0.0.1,
//! with its working directory and storage in a fresh temporary directory, and its logging config
//! (`LOG_CONFIG`) sending GELF to a socket the test holds — never the network's log host, and no
//! OpenTelemetry. The child is killed when the test ends, however it ends.

use std::net::UdpSocket;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::Duration;

use mqtt_test_broker::FakeBroker;

/// The child, and the directory it runs in; both gone when this is dropped.
struct Bridge {
    child: Child,
    dir: PathBuf,
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A logging config sending GELF to `gelf` and nowhere else, in `dir`: its path.
fn gelf_config(dir: &Path, gelf: &UdpSocket) -> PathBuf {
    let config = dir.join("test-logging.toml");
    std::fs::write(
        &config,
        format!(
            "[logging]\ndestination = \"\"\n\n[logging.gelf]\naddress = \"{}\"\n",
            gelf.local_addr().expect("the GELF socket's address")
        ),
    )
    .expect("write the logging config");
    config
}

impl Bridge {
    /// The bridge on `broker`, in a fresh directory prepared by `prepare`, logging GELF to `gelf`.
    fn start(
        tag: &str,
        broker: &FakeBroker,
        gelf: &UdpSocket,
        prepare: impl FnOnce(&Path),
    ) -> Bridge {
        Bridge::start_with(
            tag,
            broker,
            |dir| {
                prepare(dir);
                gelf_config(dir, gelf)
            },
            Stdio::null(),
            Stdio::null(),
        )
    }

    /// The bridge on `broker`, in a fresh directory prepared by `prepare`, which returns the path
    /// for `LOG_CONFIG`; its stdout and stderr as given.
    fn start_with(
        tag: &str,
        broker: &FakeBroker,
        prepare: impl FnOnce(&Path) -> PathBuf,
        stdout: Stdio,
        stderr: Stdio,
    ) -> Bridge {
        let dir =
            std::env::temp_dir().join(format!("mqtt-dmx-process-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the bridge's directory");
        let config = prepare(&dir);
        let child = Command::new(env!("CARGO_BIN_EXE_mqtt_dmx"))
            .arg(broker.address())
            .arg("--storage")
            .arg(dir.join("storage"))
            .current_dir(&dir)
            .env("LOG_CONFIG", &config)
            .env_remove("LOG_DESTINATION")
            .env_remove("LOG_LEVEL")
            .env_remove("RUST_LOG")
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .spawn()
            .expect("start the bridge");
        Bridge { child, dir }
    }

    /// Its exit status, once it has exited within `within`.
    async fn exited(&mut self, within: Duration) -> Option<ExitStatus> {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            if let Some(status) = self.child.try_wait().expect("poll the bridge") {
                return Some(status);
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

/// A GELF ingest of the test's own, on loopback.
fn gelf_socket() -> UdpSocket {
    let socket = UdpSocket::bind("127.0.0.1:0").expect("bind the GELF socket");
    socket
        .set_read_timeout(Some(Duration::from_millis(100)))
        .expect("set the GELF socket's timeout");
    socket
}

/// The GELF messages received so far.
fn received(socket: &UdpSocket, into: &mut Vec<String>) {
    let mut buffer = [0u8; 65536];
    while let Ok(n) = socket.recv(&mut buffer) {
        into.push(String::from_utf8_lossy(&buffer[..n]).into_owned());
    }
}

/// Whether a GELF message containing `text` arrives within `within`.
async fn logged(
    socket: &UdpSocket,
    messages: &mut Vec<String>,
    text: &str,
    within: Duration,
) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        received(socket, messages);
        if messages.iter().any(|m| m.contains(text)) {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn sigterm(child: &Child) {
    let status = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("run kill");
    assert!(status.success(), "kill -TERM failed");
}

/// A FIFO at `path` with nobody at its other end: whoever opens it waits there — a write until a
/// reader comes, a read until a writer does. A stalled disk, for as long as the test wants; the
/// bridge's child process is killed at the end, which ends the wait.
fn stalled_file(path: &Path) {
    let status = Command::new("mkfifo").arg(path).status().expect("run mkfifo");
    assert!(status.success(), "mkfifo {} failed", path.display());
}

/// The bridge's storage directory inside its working directory, with `name` in it stalled.
fn storage_with_stalled(dir: &Path, name: &str) {
    let storage = dir.join("storage");
    std::fs::create_dir_all(&storage).expect("create the storage directory");
    stalled_file(&storage.join(name));
}

/// A universe on loopback that never sends: a config command whose only effect outside the
/// bridge is the save of `universes.json` (written as `.universes.json.tmp`, then renamed).
const PROBE_UNIVERSE: &str = r#"{
    "description": "A probe", "controller": "127.0.0.1", "net": 0, "subnet": 0, "universe": 0,
    "channels": 8, "disable_send": true
}"#;

/// How long the bridge may take to exit after SIGTERM with a thread stuck in a write: its service
/// stop (5 s), the runtime's drain (1 s) and the log's flush, with room to spare.
const STOP_BOUND: Duration = Duration::from_secs(15);

/// A config save that cannot finish — the disk stalls — must not hold the bridge's commands: the
/// write is the subscriber's to start, never to wait on (no-hang F1, Store 3b re-review X1). On the
/// old code the subscriber wrote the file itself, on an async worker, and took no command after.
#[tokio::test(flavor = "multi_thread")]
async fn a_stalled_config_write_holds_up_no_command() {
    let test = async {
        let broker = FakeBroker::start().await;
        let gelf = gelf_socket();
        let _bridge = Bridge::start("write-commands", &broker, &gelf, |dir| {
            storage_with_stalled(dir, ".universes.json.tmp")
        });
        assert!(
            broker.wait_for_subscription("DMX/#", Duration::from_secs(20)).await,
            "the bridge never subscribed"
        );
        // Its save opens the FIFO and waits there.
        assert!(broker.send("DMX/Universe/Probe", PROBE_UNIVERSE));
        // Not a DMX subtopic: an error report, once the subscriber reaches it.
        assert!(broker.send("DMX/Bogus", "{}"));
        assert!(
            broker
                .wait_until(Duration::from_secs(10), |b| !b.received_on("DMX/Error").is_empty())
                .await,
            "the bridge took no command after a config write stalled — the subscriber waited on the disk"
        );
    };
    tokio::time::timeout(Duration::from_secs(60), test)
        .await
        .expect("the test itself ran out of time");
}

/// SIGTERM while a config write is stuck on a stalled disk: the bridge still exits, within its
/// bound. A thread inside a synchronous write cannot be interrupted, and dropping a tokio runtime
/// waits for every such thread without limit (no-hang F1, Store 3b re-review X1): on the old code
/// the service stopped, and the process then never ended.
#[tokio::test(flavor = "multi_thread")]
async fn a_stalled_config_write_does_not_hold_up_the_stop() {
    let test = async {
        let broker = FakeBroker::start().await;
        let gelf = gelf_socket();
        let mut messages = Vec::new();
        let mut bridge = Bridge::start("write-stop", &broker, &gelf, |dir| {
            storage_with_stalled(dir, ".universes.json.tmp")
        });
        assert!(
            broker.wait_for_subscription("DMX/#", Duration::from_secs(20)).await,
            "the bridge never subscribed"
        );
        assert!(broker.send("DMX/Universe/Probe", PROBE_UNIVERSE));
        // The save is stuck once it passes its bound and says so; the old code never says so, and
        // has long since reached the write by then.
        let stalled = logged(&gelf, &mut messages, "config_save_failed", Duration::from_secs(15)).await;

        sigterm(&bridge.child);
        let status = bridge.exited(STOP_BOUND).await.unwrap_or_else(|| {
            panic!(
                "the bridge did not exit within {STOP_BOUND:?} of SIGTERM while a config write was stalled — \
                 the runtime waited on the stuck write"
            )
        });
        assert!(status.success(), "the bridge did not stop cleanly ({status})");
        assert!(stalled, "the config write never reported its stall (config_save_failed), so the test proved nothing");
    };
    tokio::time::timeout(Duration::from_secs(90), test)
        .await
        .expect("the test itself ran out of time");
}

/// A config read stuck at startup — the disk stalls — must not keep the bridge from its broker
/// for good: the read is bounded, and past its bound the bridge starts without its saved config,
/// which the broker's retained configs restore (no-hang F1). On the old code `main` read the files
/// itself, and never got further.
#[tokio::test(flavor = "multi_thread")]
async fn a_stalled_config_read_at_startup_does_not_keep_the_bridge_off_its_broker() {
    let test = async {
        let broker = FakeBroker::start().await;
        let gelf = gelf_socket();
        let mut messages = Vec::new();
        let _bridge = Bridge::start("read-start", &broker, &gelf, |dir| {
            storage_with_stalled(dir, "universes.json")
        });
        assert!(
            broker.wait_for_subscription("DMX/#", Duration::from_secs(30)).await,
            "the bridge never reached its broker — its startup waited on a stalled config read"
        );
        assert!(
            logged(&gelf, &mut messages, "saved configuration", Duration::from_secs(2)).await,
            "the bridge started without its saved config and never said so"
        );
    };
    tokio::time::timeout(Duration::from_secs(60), test)
        .await
        .expect("the test itself ran out of time");
}

/// SIGTERM while the startup's config read is stuck: the bridge exits at once, not after the
/// read's bound — the stop races the startup (no-hang F1, F3). On the old code it never exited.
#[tokio::test(flavor = "multi_thread")]
async fn a_stop_during_a_stalled_config_read_at_startup_is_prompt() {
    // Well under the read's 10 s bound, and well over the runtime's 1 s drain.
    const PROMPT: Duration = Duration::from_secs(6);
    let test = async {
        let broker = FakeBroker::start().await;
        let gelf = gelf_socket();
        let mut messages = Vec::new();
        let mut bridge = Bridge::start("read-stop", &broker, &gelf, |dir| {
            storage_with_stalled(dir, "universes.json")
        });
        assert!(
            logged(&gelf, &mut messages, "Starting mqtt_dmx", Duration::from_secs(20)).await,
            "the bridge never started"
        );
        // Inside the stalled read.
        tokio::time::sleep(Duration::from_secs(1)).await;
        sigterm(&bridge.child);
        let status = bridge.exited(PROMPT).await.unwrap_or_else(|| {
            panic!("the bridge did not exit within {PROMPT:?} of SIGTERM during a stalled config read at startup")
        });
        assert!(status.success(), "the bridge did not stop cleanly ({status})");
    };
    tokio::time::timeout(Duration::from_secs(60), test)
        .await
        .expect("the test itself ran out of time");
}

/// systemd stops a unit with SIGTERM. The bridge must take it as a stop: run its bounded shutdown,
/// flush its log, and exit 0 — not die by the signal's default action, which skips both.
#[tokio::test(flavor = "multi_thread")]
async fn sigterm_stops_the_bridge_through_its_shutdown() {
    let test = async {
        let broker = FakeBroker::start().await;
        let gelf = gelf_socket();
        let mut messages = Vec::new();
        let mut bridge = Bridge::start("sigterm", &broker, &gelf, |_| {});
        assert!(
            broker
                .wait_for_subscription("DMX/#", Duration::from_secs(20))
                .await,
            "the bridge never subscribed"
        );

        sigterm(&bridge.child);
        let status = bridge
            .exited(Duration::from_secs(10))
            .await
            .expect("the bridge did not exit within 10 s of SIGTERM");
        assert!(
            status.success(),
            "the bridge did not stop cleanly on SIGTERM ({status}) — killed by the signal, its shutdown and log flush skipped"
        );
        assert!(
            logged(
                &gelf,
                &mut messages,
                "Service stopped",
                Duration::from_secs(2)
            )
            .await,
            "the bridge exited without logging its stop"
        );
    };
    tokio::time::timeout(Duration::from_secs(60), test)
        .await
        .expect("the test itself ran out of time");
}

/// A working directory where the log file cannot be created — `logs` is a file — costs the file
/// destination alone: the bridge pins `on_destination_error = skip` in code (its `logging.toml`,
/// found by an upward search from the working directory, may be missing or say otherwise), so it
/// starts, and logs to its other destinations.
#[tokio::test(flavor = "multi_thread")]
async fn a_log_destination_that_cannot_start_costs_that_destination_alone() {
    let test = async {
        let broker = FakeBroker::start().await;
        let gelf = gelf_socket();
        let mut messages = Vec::new();
        let _bridge = Bridge::start("skip", &broker, &gelf, |dir| {
            std::fs::write(dir.join("logs"), "a file where the log directory would go")
                .expect("write the blocking file");
        });
        assert!(
            broker
                .wait_for_subscription("DMX/#", Duration::from_secs(20))
                .await,
            "the bridge never subscribed — a log destination that could not start stopped it"
        );
        assert!(
            logged(&gelf, &mut messages, "Starting mqtt_dmx", Duration::from_secs(5)).await,
            "no log reached GELF — a log destination that could not start took logging down with it"
        );
    };
    tokio::time::timeout(Duration::from_secs(60), test)
        .await
        .expect("the test itself ran out of time");
}

/// The UTC date `days` from today, as tracing-init's daily log file names it (`YYYY-MM-DD`).
fn utc_date(days: i64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("after 1970");
    // Days since 1970-01-01 to a civil date (H. Hinnant's algorithm).
    let z = (now.as_secs() / 86_400) as i64 + days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// A log destination that stalls while the logging starts — the log file a FIFO nobody reads, as
/// on a hung disk — holds up neither the stop nor the process's end: SIGTERM ends the bridge
/// cleanly about a second later (Store no-hang 3b review round 3, B1). The whole logging start runs
/// on a blocking thread raced against the stop, and the runtime's bounded end abandons that thread
/// while it still waits (F1). tracing-init gives a stalled destination up after 5 s, so on the old
/// code, which started the logging inline, a SIGTERM sent at 1 s was acted on about 4 s later. The
/// SIGTERM's own time is checked, so a late one cannot hide that wait. The FIFOs' read ends are
/// never opened.
#[tokio::test(flavor = "multi_thread")]
async fn a_stop_during_a_stalled_logging_start_is_prompt() {
    // The runtime's 1 s drain, with room; the old code's wait was about 4 s.
    const PROMPT: Duration = Duration::from_millis(2500);
    let test = async {
        let broker = FakeBroker::start().await;
        let gelf = gelf_socket();
        let spawned = std::time::Instant::now();
        let mut bridge = Bridge::start_with(
            "logging-start",
            &broker,
            |dir| {
                let logs = dir.join("logs");
                std::fs::create_dir_all(&logs).expect("create the log directory");
                // Today's file, and tomorrow's should the date turn meanwhile (UTC, as tracing-init names it).
                for days in [0, 1] {
                    stalled_file(&logs.join(format!("dmx.{}.log", utc_date(days))));
                }
                gelf_config(dir, &gelf)
            },
            Stdio::null(),
            Stdio::null(),
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(bridge.child.try_wait().expect("poll the bridge").is_none(), "the bridge ended on its own");
        sigterm(&bridge.child);
        let sent = spawned.elapsed();
        assert!(sent < Duration::from_millis(1500), "inconclusive: SIGTERM went out {sent:?} after the start, near tracing-init's 5 s");
        let status = bridge.exited(PROMPT).await.unwrap_or_else(|| {
            panic!("the bridge did not exit within {PROMPT:?} of SIGTERM while its logging start was stalled")
        });
        assert!(status.success(), "the bridge did not stop cleanly ({status})");
    };
    tokio::time::timeout(Duration::from_secs(60), test)
        .await
        .expect("the test itself ran out of time");
}

/// The bridge's lifecycle output goes through its logging, whose writers drop lines rather than
/// wait, never straight to stdout or stderr: a supervisor's pipe that has stopped draining holds up
/// neither its start nor its stop (Store no-hang 3b review round 3, B2). Its stdout and stderr
/// here are one pipe the test fills first and never reads until the bridge has exited. On the old
/// code `main` printed its logging summary there, and never got further.
#[tokio::test(flavor = "multi_thread")]
async fn a_full_output_pipe_holds_up_neither_the_start_nor_the_stop() {
    use std::io::Write;
    let test = async {
        let (unread, output) = std::io::pipe().expect("a pipe");
        // Full: a thread writes into it until it blocks, and stays there until the pipe's reader
        // is dropped, after the bridge has exited.
        let mut filler = output.try_clone().expect("the pipe's write end");
        std::thread::spawn(move || {
            let block = [b'.'; 65536];
            while filler.write_all(&block).is_ok() {}
        });
        tokio::time::sleep(Duration::from_millis(500)).await;

        let broker = FakeBroker::start().await;
        let gelf = gelf_socket();
        let mut bridge = Bridge::start_with(
            "full-pipe",
            &broker,
            |dir| gelf_config(dir, &gelf),
            Stdio::from(output.try_clone().expect("the pipe's write end")),
            Stdio::from(output),
        );
        assert!(
            broker.wait_for_subscription("DMX/#", Duration::from_secs(20)).await,
            "the bridge never subscribed — it waited on its full stdout or stderr"
        );
        sigterm(&bridge.child);
        let status = bridge
            .exited(STOP_BOUND)
            .await
            .unwrap_or_else(|| panic!("the bridge did not exit within {STOP_BOUND:?} of SIGTERM with its output pipe full"));
        assert!(status.success(), "the bridge did not stop cleanly ({status})");
        drop(unread);
    };
    tokio::time::timeout(Duration::from_secs(60), test)
        .await
        .expect("the test itself ran out of time");
}
