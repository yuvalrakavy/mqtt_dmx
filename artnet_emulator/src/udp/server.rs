use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::RwLock;

use crate::state::{EmulatorState, StateVersionNotifier, UniverseKey};
use crate::udp::log::SharedLog;

pub type SharedState = Arc<RwLock<EmulatorState>>;

const ARTNET_HEADER: &[u8; 8] = b"Art-Net\0";
const ARTNET_OPCODE_DMX: u16 = 0x5000;

/// Start the UDP server, returns the local address it bound to
pub async fn start_server(
    bind_addr: &str,
    state: SharedState,
    log: SharedLog,
    version_notifier: StateVersionNotifier,
) -> std::net::SocketAddr {
    let socket = UdpSocket::bind(bind_addr)
        .await
        .unwrap_or_else(|e| panic!("Failed to bind UDP socket to {bind_addr}: {e}"));
    let addr = socket.local_addr().unwrap();

    tracing::info!("ArtNet UDP server listening on {addr}");

    tokio::spawn(udp_server_loop(socket, state, log, version_notifier));

    addr
}

async fn udp_server_loop(
    socket: UdpSocket,
    state: SharedState,
    log: SharedLog,
    version_notifier: StateVersionNotifier,
) {
    let mut buf = [0u8; 1024];

    loop {
        let (len, src) = match socket.recv_from(&mut buf).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("UDP receive error: {e}");
                continue;
            }
        };

        let packet = &buf[..len];

        match parse_artnet_packet(packet) {
            Ok((key, dmx_data)) => {
                let channel_count = dmx_data.len();
                log.push(format!(
                    "From {src}: {} — {channel_count} channels",
                    key,
                )).await;

                let mut s = state.write().await;
                s.update_universe(key, dmx_data);
                version_notifier.notify(s.version);
            }
            Err(e) => {
                tracing::warn!("Invalid ArtNet packet from {src}: {e}");
            }
        }
    }
}

/// Parse an ArtNet DMX Output packet
/// Returns (UniverseKey, &[u8] DMX channel data)
fn parse_artnet_packet(packet: &[u8]) -> Result<(UniverseKey, &[u8]), &'static str> {
    // Minimum packet size: 8 (header) + 2 (opcode) + 2 (version) + 1 (seq) + 1 (phys) + 1 (sub_uni) + 1 (net) + 2 (length) = 18
    if packet.len() < 18 {
        return Err("Packet too short");
    }

    // Verify Art-Net header
    if &packet[0..8] != ARTNET_HEADER {
        return Err("Invalid Art-Net header");
    }

    // OpCode (little-endian at offset 8)
    let opcode = u16::from_le_bytes([packet[8], packet[9]]);
    if opcode != ARTNET_OPCODE_DMX {
        return Err("Not a DMX Output packet");
    }

    // SubUniverse: high nibble = subnet, low nibble = universe
    let sub_uni = packet[14];
    let subnet = (sub_uni >> 4) & 0x0F;
    let universe = sub_uni & 0x0F;
    let net = packet[15];

    // Data length (big-endian at offset 16)
    let data_len = u16::from_be_bytes([packet[16], packet[17]]) as usize;
    let data_end = 18 + data_len.min(packet.len() - 18);
    let dmx_data = &packet[18..data_end];

    let key = UniverseKey { net, subnet, universe };
    Ok((key, dmx_data))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_artnet_packet(net: u8, subnet: u8, universe: u8, data: &[u8]) -> Vec<u8> {
        let mut packet = Vec::new();
        packet.extend_from_slice(b"Art-Net\0"); // header
        packet.extend_from_slice(&0x5000u16.to_le_bytes()); // opcode
        packet.extend_from_slice(&[0x00, 0x0e]); // protocol version
        packet.push(0); // sequence
        packet.push(0); // physical
        packet.push((subnet << 4) | (universe & 0x0F)); // sub_uni
        packet.push(net); // net
        packet.extend_from_slice(&(data.len() as u16).to_be_bytes()); // length
        packet.extend_from_slice(data); // DMX data
        packet
    }

    #[test]
    fn test_parse_valid_packet() {
        let data = vec![255, 128, 0, 64];
        let packet = build_artnet_packet(0, 1, 2, &data);
        let (key, dmx_data) = parse_artnet_packet(&packet).unwrap();
        assert_eq!(key.net, 0);
        assert_eq!(key.subnet, 1);
        assert_eq!(key.universe, 2);
        assert_eq!(dmx_data, &[255, 128, 0, 64]);
    }

    #[test]
    fn test_parse_invalid_header() {
        let packet = b"Not-Art\x00\x00\x50\x00\x0e\x00\x00\x00\x00\x00\x01\x00";
        assert!(parse_artnet_packet(packet).is_err());
    }

    #[test]
    fn test_parse_wrong_opcode() {
        let mut packet = build_artnet_packet(0, 0, 0, &[0]);
        packet[8] = 0x00; // change opcode to non-DMX
        packet[9] = 0x00;
        assert!(parse_artnet_packet(&packet).is_err());
    }

    #[test]
    fn test_parse_too_short() {
        let packet = b"Art-Net\x00\x00\x50";
        assert!(parse_artnet_packet(packet).is_err());
    }
}
