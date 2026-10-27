//! Art-Net output prepared by the serialized lighting owner. Packet construction
//! has no socket, MQTT or filesystem dependency.
use crate::{engine::Channel, installation::Installation};
use dmx_protocol::Error;
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    net::{SocketAddr, UdpSocket},
};

/// Local nonblocking sockets owned by the lighting service. No DNS, acknowledgments
/// or broker activity is involved. Errors are returned without stopping playback.
#[derive(Default)]
pub struct DatagramSender {
    ipv4: Option<UdpSocket>,
    ipv6: Option<UdpSocket>,
}
impl DatagramSender {
    pub fn send(&mut self, datagram: &Datagram) -> io::Result<()> {
        let slot = if datagram.destination.is_ipv4() {
            &mut self.ipv4
        } else {
            &mut self.ipv6
        };
        if slot.is_none() {
            let bind = if datagram.destination.is_ipv4() {
                "0.0.0.0:0"
            } else {
                "[::]:0"
            };
            let socket = UdpSocket::bind(bind)?;
            socket.set_nonblocking(true)?;
            *slot = Some(socket);
        }
        let bytes = slot
            .as_ref()
            .expect("socket initialized")
            .send_to(&datagram.bytes, datagram.destination)?;
        if bytes != datagram.bytes.len() {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "incomplete Art-Net datagram",
            ));
        }
        tracing::trace!(target: "detail.artnet_tx", destination = %datagram.destination,
            bytes, sequence = ?datagram.bytes.get(12), "Art-Net output sent");
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Datagram {
    pub destination: SocketAddr,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Address {
    destination: SocketAddr,
    net: u8,
    subnet: u8,
    universe: u8,
}
impl Address {
    fn channel(channel: &Channel) -> Self {
        let channel = channel.clone().canonical();
        Self {
            destination: channel.endpoint,
            net: channel.net,
            subnet: channel.subnet,
            universe: channel.universe,
        }
    }
}
#[derive(Clone)]
struct Frame {
    levels: [u8; 512],
    sequence: u8,
}
impl Frame {
    fn packet(&mut self, address: &Address) -> Datagram {
        self.sequence = if self.sequence == 255 {
            1
        } else {
            self.sequence + 1
        };
        let mut bytes = Vec::with_capacity(530);
        bytes.extend_from_slice(b"Art-Net\0");
        bytes.extend_from_slice(&0x5000u16.to_le_bytes());
        bytes.extend_from_slice(&14u16.to_be_bytes());
        bytes.extend_from_slice(&[
            self.sequence,
            0,
            (address.subnet << 4) | address.universe,
            address.net,
        ]);
        bytes.extend_from_slice(&512u16.to_be_bytes());
        bytes.extend_from_slice(&self.levels);
        Datagram {
            destination: address.destination,
            bytes,
        }
    }
}

pub struct Frames {
    universes: BTreeMap<Address, Frame>,
}
impl Frames {
    pub fn reconfigure(
        &mut self,
        installation: &Installation,
        released: &[Channel],
    ) -> Result<Vec<Datagram>, Error> {
        let mut previous = Self {
            universes: self.universes.clone(),
        };
        let clearing = previous.clear_released(released)?;
        let mut next = Self::new(installation)?;
        for (address, frame) in &mut next.universes {
            if let Some(old) = previous.universes.get(address) {
                frame.sequence = old.sequence;
            }
        }
        *self = next;
        Ok(clearing)
    }
    pub fn new(installation: &Installation) -> Result<Self, Error> {
        let universes = installation
            .configuration()
            .content
            .universes
            .values()
            .map(|universe| {
                let address = Address::channel(&Channel {
                    endpoint: universe.endpoint,
                    net: universe.net,
                    subnet: universe.subnet,
                    universe: universe.universe,
                    channel: 0,
                });
                (
                    address,
                    Frame {
                        levels: [0; 512],
                        sequence: 0,
                    },
                )
            })
            .collect();
        let mut frames = Self { universes };
        frames.render(installation)?;
        Ok(frames)
    }
    /// Update owned channels from the actual engine. A mismatched physical
    /// universe set rejects the whole update, leaving previous output intact.
    pub fn render(&mut self, installation: &Installation) -> Result<(), Error> {
        let expected: BTreeSet<_> = installation
            .configuration()
            .content
            .universes
            .values()
            .map(|universe| {
                Address::channel(&Channel {
                    endpoint: universe.endpoint,
                    net: universe.net,
                    subnet: universe.subnet,
                    universe: universe.universe,
                    channel: 0,
                })
            })
            .collect();
        if expected != self.universes.keys().cloned().collect() {
            return Err(Error::Invalid(
                "output universe set differs from the installed configuration".into(),
            ));
        }
        let mut candidate = self.universes.clone();
        for (array, channel) in installation.channels() {
            let frame = candidate
                .get_mut(&Address::channel(channel))
                .ok_or_else(|| {
                    Error::Invalid("owned output channel has no configured universe".into())
                })?;
            let output = frame
                .levels
                .get_mut(usize::from(channel.channel))
                .ok_or_else(|| {
                    Error::Invalid("owned output channel exceeds the physical universe".into())
                })?;
            *output = installation.output(array, channel);
        }
        self.universes = candidate;
        Ok(())
    }
    /// One immutable frame per configured universe, for the owner's 50 ms tick.
    pub fn datagrams(&mut self) -> Vec<Datagram> {
        self.universes
            .iter_mut()
            .map(|(address, frame)| frame.packet(address))
            .collect()
    }
    /// Produce clearing frames on the OLD mapping at the activation boundary,
    /// before replacing it. Never erase channels that have not been released.
    pub fn clear_released(&mut self, channels: &[Channel]) -> Result<Vec<Datagram>, Error> {
        let mut candidate = self.universes.clone();
        let mut affected = BTreeSet::new();
        for channel in channels {
            let address = Address::channel(channel);
            let frame = candidate.get_mut(&address).ok_or_else(|| {
                Error::Invalid("released channel has no previous output universe".into())
            })?;
            let output = frame
                .levels
                .get_mut(usize::from(channel.channel))
                .ok_or_else(|| {
                    Error::Invalid("released channel exceeds the physical universe".into())
                })?;
            *output = 0;
            affected.insert(address);
        }
        let packets = affected
            .into_iter()
            .map(|address| {
                candidate
                    .get_mut(&address)
                    .expect("affected universe exists")
                    .packet(&address)
            })
            .collect();
        self.universes = candidate;
        Ok(packets)
    }
}
