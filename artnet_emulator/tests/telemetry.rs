#![cfg(feature = "server")]

use serde_json::Value;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Emulator(Child);

impl Drop for Emulator {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn receive_until(
    socket: &UdpSocket,
    messages: &mut Vec<Value>,
    ready: impl Fn(&Value) -> bool,
) -> Value {
    let until = Instant::now() + Duration::from_secs(10);
    let mut buffer = [0; 65536];
    while Instant::now() < until {
        match socket.recv(&mut buffer) {
            Ok(size) => {
                let message: Value =
                    serde_json::from_slice(&buffer[..size]).expect("valid GELF JSON");
                let done = ready(&message);
                messages.push(message.clone());
                if done {
                    return message;
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => panic!("GELF receive: {error}"),
        }
    }
    panic!("emulator telemetry did not reach the expected state; received {messages:?}");
}

fn exercise_receiver(trace_frames: bool) {
    let collector = UdpSocket::bind("127.0.0.1:0").unwrap();
    collector
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("logging.toml");
    let filter = if trace_frames {
        "info,detail.artnet_rx=trace"
    } else {
        "info"
    };
    std::fs::write(
        &config,
        format!(
        "[logging]\ndestination = \"g\"\nfilter = \"{filter}\"\n[logging.gelf]\naddress = \"{}\"\n",
        collector.local_addr().unwrap(),
    ),
    )
    .unwrap();
    let web_reservation = TcpListener::bind("127.0.0.1:0").unwrap();
    let web_address = web_reservation.local_addr().unwrap();
    drop(web_reservation);
    let mut emulator = Emulator(
        Command::new(env!("CARGO_BIN_EXE_artnet_emulator"))
            .args([
                "--port",
                "0",
                "--web-port",
                &web_address.port().to_string(),
                "--bind",
                "127.0.0.1",
                "--logging-config",
            ])
            .arg(&config)
            .env_remove("PORT")
            .env_remove("IP")
            .env_remove("RUST_LOG")
            .env_remove("LOG_LEVEL")
            .env_remove("LOG_DESTINATION")
            .env_remove("LOG_CONFIG")
            // A cargo-built server has no dx-generated public/ directory. Supply
            // an explicit empty asset directory; this test exercises server telemetry.
            .env("DIOXUS_PUBLIC_PATH", directory.path())
            .current_dir(directory.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    // Do not pass on UDP startup logs emitted before a later web-startup panic.
    let until = Instant::now() + Duration::from_secs(10);
    let mut web = loop {
        assert!(
            emulator.0.try_wait().unwrap().is_none(),
            "emulator exited during startup"
        );
        if let Ok(stream) = TcpStream::connect_timeout(&web_address, Duration::from_millis(100)) {
            break stream;
        }
        assert!(
            Instant::now() < until,
            "emulator web listener did not start"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    web.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    web.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut response = [0; 512];
    let count = web.read(&mut response).unwrap();
    assert!(
        response[..count].starts_with(b"HTTP/1.1 200"),
        "emulator web response: {:?}",
        String::from_utf8_lossy(&response[..count])
    );
    let mut messages = Vec::new();
    let receiver = receive_until(&collector, &mut messages, |message| {
        message["short_message"] == "ArtNet UDP receiver listening"
    });
    assert_eq!(receiver["_service"], "artnet_emulator");
    let endpoint: SocketAddr = receiver["_address"].as_str().unwrap().parse().unwrap();
    assert!(endpoint.ip().is_loopback());
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    // Art-Net producer layout: manager.rs Universe::new, opcode LE, length BE.
    let frame = [
        b'A', b'r', b't', b'-', b'N', b'e', b't', 0, 0, 0x50, 0, 14, 7, 0, 0x12, 3, 0, 4, 10, 20,
        30, 40,
    ];
    sender.send_to(&frame, endpoint).unwrap();
    let discovered = receive_until(&collector, &mut messages, |message| {
        message["short_message"] == "ArtNet universe discovered"
    });
    assert_eq!(discovered["_net"], 3);
    assert_eq!(discovered["_subnet"], 1);
    assert_eq!(discovered["_universe"], 2);
    assert_eq!(discovered["_channels"], 4);
    let frames: Vec<_> = messages
        .iter()
        .filter(|message| message["short_message"] == "ArtNet frame received")
        .collect();
    assert_eq!(frames.len(), usize::from(trace_frames));
    if let Some(frame) = frames.first() {
        assert_eq!(frame["_sequence"], 7);
        assert_eq!(frame["_values"], "[10, 20, 30, 40]");
    }
    sender.send_to(b"bad", endpoint).unwrap();
    let rejected = receive_until(&collector, &mut messages, |message| {
        message["short_message"] == "Invalid ArtNet packet"
    });
    assert_eq!(rejected["_kind"], "protocol_invalid");
    assert_eq!(rejected["_bytes"], 3);
    assert!(
        emulator.0.try_wait().unwrap().is_none(),
        "emulator exited after handling the packets"
    );
}

#[test]
fn reports_packets_and_decode_failures_to_selected_collector() {
    exercise_receiver(true);
}

#[test]
fn normal_playback_has_no_per_frame_baseline_telemetry() {
    exercise_receiver(false);
}
