use std::{net::{IpAddr, UdpSocket}, collections::HashMap, iter::repeat, fmt::Debug, mem, sync::{Weak, Arc}, time::Duration};
use tokio::{select, sync::mpsc::Receiver, time::interval};
use tokio_util::sync::CancellationToken;
use log::info;

use super::ArtnetError;
use crate::{defs::UniverseDefinition, defs::{self, TargetValue}, dmx::*, messages::{ToArtnetManagerMessage, ToMqttPublisherMessage}};

//NOTE: Actual Artnet packet sending is commented out

#[derive(Debug)]
pub (super) struct ArtnetController {
    socket: UdpSocket,
}

#[derive(Debug)]
pub (super)struct Universe {
    description: String,

    controller: Arc<ArtnetController>,
    packet_bytes: Vec<u8>,
    modified: bool,
    log: bool,
    disable_send: bool,
    non_modified_ticks: usize,      // Number of ticks in which this universe was not modified (used to determine when to send a packet)
}

pub trait EffectNodeRuntime : Debug + Send {
    fn tick(&mut self, artnet_manager: &mut ArtnetManager) -> Result<(), ArtnetError>;
    fn is_done(&self) -> bool;
}

pub struct ArtnetManager {
    pub (super) universes: HashMap<String, Universe>,
    pub (super) controllers: HashMap<IpAddr, Weak<ArtnetController>>,
    active_effects: HashMap<String, Box<dyn EffectNodeRuntime>>,
    #[cfg(test)]
    pub (super) set_channel_log: Vec<ChannelValue>,
}

pub (super) const DMX_DATA_OFFSET: usize = 18;
const DMX_SEQ_OFFSET: usize = 12;
const DMX_UDP_PORT: u16 = 0x1936;
const ARTNET_OPCODE_OUTPUT: u16 = 0x5000;
const TICK_DURATION: Duration = Duration::from_millis(50);
const SEND_UNMODIFIED_UNIVERSE_EVERY: usize = 20*4;     // 20 ticks per second, send every 4 seconds

impl ArtnetManager {
    pub fn new() -> ArtnetManager {
        ArtnetManager {
            universes: HashMap::new(),
            controllers: HashMap::new(),
            active_effects: HashMap::new(),
            #[cfg(test)]
            set_channel_log: Vec::new(),
        }
    }

    pub (super) fn add_universe(&mut self, universe_id: &str, definition: UniverseDefinition) -> Result<(), ArtnetError> {
        let controller = match self.controllers.get(&definition.controller) {
            Some(c) => c.upgrade().unwrap(),
            None => {
                let controller = Arc::new(ArtnetController::new(&definition.controller)?);
                self.controllers.insert(definition.controller, Arc::downgrade(&controller));
                controller
            }
        };

        let universe = Universe::new(controller, universe_id, definition)?;
        self.universes.insert(universe_id.to_owned(), universe);

        Ok(())
    }

    pub (super) fn remove_universe(&mut self, universe_id: &str) -> Result<(), ArtnetError> {
         self.universes.remove(universe_id).ok_or_else(|| ArtnetError::InvalidUniverse(universe_id.to_string()))?;

        let to_remove = self.controllers.iter().filter(|(_, c)| c.upgrade().is_none()).map(|(ip, _)| *ip).collect::<Vec<IpAddr>>();
        
        for ip in to_remove.iter() {
            self.controllers.remove(ip);
        }
    
        Ok(())
    }

    fn start_effect(&mut self, effect_id: &str, effect: Box<dyn EffectNodeRuntime>) -> Result<(), ArtnetError> {
        info!("Starting effect {}: {:?}", effect_id, effect);
        self.active_effects.insert(effect_id.to_owned(), effect);
        Ok(())
    }

    fn stop_effect(&mut self, effect_id: &str) -> Result<(), ArtnetError> {
        info!("Stopping effect {}", effect_id);
        self.active_effects.remove(effect_id);
        Ok(())
    }

    fn tick(&mut self) -> Result<(), ArtnetError> {
        let mut active_effects = mem::take(&mut self.active_effects);
        let mut completed_effect: Vec<String> = Vec::new();

        for (effect_id, effect) in active_effects.iter_mut() {
            effect.tick(self)?;

            if effect.is_done() {
                completed_effect.push(effect_id.clone());
            }
        }

        for id in completed_effect {
            info!("Effect {} completed", id);
            active_effects.remove(&id);
        }

        self.active_effects = active_effects;       // Move it back
        Ok(())
    }


    pub fn set_channel(&mut self, universe_id: &str, v: &ChannelValue) -> Result<(), ArtnetError> {
        info!("Setting channel {} to {:?}", v.channel, v.value);

        match self.universes.get_mut(universe_id) {
            Some(u) => {
                if u.log {
                    #[cfg(test)]
                    self.set_channel_log.push(v.clone());
                }
                u.set_channel(v)
            },
            None => Err(ArtnetError::InvalidUniverse(universe_id.to_string())),
        }
    }

    pub fn get_channel(&self, universe_id: &str, channel_definition: &ChannelDefinition) -> Result<ChannelValue, ArtnetError> {
        match self.universes.get(universe_id) {
            Some(u) => u.get_channel(channel_definition),
            None => Err(ArtnetError::InvalidUniverse(universe_id.to_string())),
        }
    }

    fn send_modified_universes(&mut self) -> Result<(), ArtnetError> {
        for (universe_id, universe) in self.universes.iter_mut() {
            if !universe.modified {
                universe.non_modified_ticks += 1;
                if universe.non_modified_ticks >= SEND_UNMODIFIED_UNIVERSE_EVERY {
                    universe.modified = true;
                }
            }

            if universe.modified {
                info!("Sending packet to {}", universe_id);
                universe.send()?;
            }
        }
        Ok(())
    }

    fn set_channels(&mut self, parameters: &defs::SetChannelsParameters) -> Result<(), ArtnetError> {
        let mut target = parameters.target.parse::<TargetValue>()?;
        let channels = parameters.channels.split(',').map(|c| c.parse::<ChannelDefinition>()).collect::<Result<Vec<ChannelDefinition>, _>>()?;

        if let Some(dimming_amount) = parameters.dimming_amount {
            target = target.get_dimmed_value(dimming_amount);
        }

        for channel_definition in channels.iter() {
            let channel_value = target.get(channel_definition.channel_type);

            if let Some(channel_value) = channel_value {
                let channel_value = ChannelValue {
                    channel: channel_definition.channel,
                    value: channel_value,
                };
                self.set_channel(&parameters.universe_id, &channel_value)?;
            }
            else {
                return Err(ArtnetError::MissingTargetValue(channel_definition.to_string(), parameters.target.to_string()))
            }
        }

        Ok(())
    }

    fn handle_message(&mut self, message: ToArtnetManagerMessage) {
        match message {
            ToArtnetManagerMessage::AddUniverse(universe_id, definition, reply_tx) => 
                reply_tx.send(self.add_universe(&universe_id, definition)).unwrap(),
            ToArtnetManagerMessage::RemoveUniverse(universe_id, sender) =>
                sender.send(self.remove_universe(&universe_id)).unwrap(),
            ToArtnetManagerMessage::StartEffect(effect_id, effect_node_runtime, reply_tx) => 
                reply_tx.send(self.start_effect(&effect_id, effect_node_runtime)).unwrap(),
            ToArtnetManagerMessage::StopEffect(effect_id, sender) =>
                sender.send(self.stop_effect(&effect_id)).unwrap(),
            ToArtnetManagerMessage::SetChannels(parameters, sender) =>
                sender.send(self.set_channels(&parameters)).unwrap(),
        }
    }

    pub async fn run(&mut self, cancel: CancellationToken, mut receiver: Receiver<ToArtnetManagerMessage>, to_mqtt_publisher: tokio::sync::mpsc::Sender<ToMqttPublisherMessage>) {
        // Set tick timer
        let mut tick_timer = interval(TICK_DURATION);

        loop {
            select! {
                _ = cancel.cancelled() => break,

                _ = tick_timer.tick() => {
                    if let Err(e) = self.tick() {
                        to_mqtt_publisher.send(ToMqttPublisherMessage::Error(e.to_string())).await.unwrap();
                    }

                    if let Err(e) = self.send_modified_universes() {
                        to_mqtt_publisher.send(ToMqttPublisherMessage::Error(e.to_string())).await.unwrap();
                    }
                },

                message = receiver.recv() => match message {
                    None => break,
                    Some(message) => self.handle_message(message),
                },
            }
        }

        info!("ArtnetManager stopped");
    }
}

impl ArtnetController {
    pub fn new(controller: &IpAddr) -> Result<ArtnetController, ArtnetError> {
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        socket.connect((*controller, DMX_UDP_PORT))?;

        Ok(ArtnetController {
            socket,
        })
    }

    pub fn send(&self, packet_bytes: &[u8]) -> Result<(), ArtnetError> {
        self.socket.send(packet_bytes)?;
        Ok(())
    }   
}

impl Universe {
    pub fn new(controller: Arc<ArtnetController>, universe_id: &str, definition: UniverseDefinition) -> Result<Universe, ArtnetError> {
        if definition.universe > 15 {
            return Err(ArtnetError::InvalidUniverseNumber(definition.universe));
        }
        if definition.subnet > 15 {
            return Err(ArtnetError::InvalidSubnet(definition.subnet));
        }
        if definition.net > 127 {
            return Err(ArtnetError::InvalidNet(definition.net));
        }
        if definition.channels > 512 {
            return Err(ArtnetError::TooManyChannels(definition.channels));
        }

        let channel_count = (definition.channels + 1) as usize & !1;        // Round up to even number of channels
        let mut packet_bytes = Vec::<u8>::with_capacity(channel_count + DMX_DATA_OFFSET);

        packet_bytes.append(&mut vec![b'A', b'r', b't', b'-', b'N', b'e', b't', 0x00]);
        packet_bytes.push((ARTNET_OPCODE_OUTPUT & 0xff) as u8);
        packet_bytes.push((ARTNET_OPCODE_OUTPUT >> 8) as u8);
        packet_bytes.push(0x00);                // Protocol version Hi
        packet_bytes.push(0x14);                // Protocol version Lo
        packet_bytes.push(0x00);                // Sequence
        packet_bytes.push(0x00);                // Physical
        packet_bytes.push(definition.subnet << 4 | definition.universe); // Subuniverse
        packet_bytes.push(definition.net);   // net
        packet_bytes.push((channel_count >> 8) as u8); // Length Hi
        packet_bytes.push((channel_count & 0xff) as u8); // Length Lo

        assert_eq!(packet_bytes.len(), DMX_DATA_OFFSET);
        packet_bytes.extend(repeat(0x00).take(channel_count));

        Ok(Universe {
            description: format!("{0} ({1})", universe_id, definition.description),
            controller,
            log: definition.log,
            disable_send: definition.disable_send,
            packet_bytes,
            modified: false,
            non_modified_ticks: 0,
        })     
    }

    #[cfg(test)]
    pub (super) fn get_packet_bytes(&self) -> &Vec<u8> {
        &self.packet_bytes
    }

    fn get_channel_count(&self) -> u16 {
        (self.packet_bytes.len() - DMX_DATA_OFFSET) as u16
    }

    pub fn set_channel(&mut self, v: &ChannelValue) -> Result<(), ArtnetError> {
        let value_bytes = match v.value {
            DimmerValue::Rgb(_, _, _) => 3,
            DimmerValue::TriWhite(_, _, _) => 3,
            DimmerValue::Single(_) => 1,
        };

        if DMX_DATA_OFFSET + v.channel as usize + value_bytes > self.packet_bytes.len() {
            return Err(ArtnetError::InvalidChannel(self.description.clone(), v.channel, self.get_channel_count()));
        }

        let offset = DMX_DATA_OFFSET + v.channel as usize;

        match v.value {
            DimmerValue::Rgb(r, g, b) => {
                self.packet_bytes[offset] = r;
                self.packet_bytes[offset + 1] = g;
                self.packet_bytes[offset + 2] = b;
            },
            DimmerValue::TriWhite(w1, w2, w3) => {
                self.packet_bytes[offset] = w1;
                self.packet_bytes[offset + 1] = w2;
                self.packet_bytes[offset + 2] = w3;
            },
            DimmerValue::Single(v) => {
                self.packet_bytes[offset] = v;
            },
        }

        self.modified = true;
        Ok(())
    }

    pub fn get_channel(&self, channel_definition: &ChannelDefinition) -> Result<ChannelValue, ArtnetError> {
        let value_bytes = match channel_definition.channel_type {
            ChannelType::Rgb => 3,
            ChannelType::TriWhite => 3,
            ChannelType::Single => 1,
        };

        if DMX_DATA_OFFSET + channel_definition.channel as usize + value_bytes > self.packet_bytes.len() {
            return Err(ArtnetError::InvalidChannel(self.description.clone(), channel_definition.channel, self.get_channel_count()));
        }

        let offset = DMX_DATA_OFFSET + channel_definition.channel as usize;

        match channel_definition.channel_type {
            ChannelType::Rgb => {
                Ok(ChannelValue {
                    channel: channel_definition.channel,
                    value: DimmerValue::Rgb(self.packet_bytes[offset], self.packet_bytes[offset + 1], self.packet_bytes[offset + 2]),
                })
            },
            ChannelType::TriWhite => {
                Ok(ChannelValue {
                    channel: channel_definition.channel,
                    value: DimmerValue::TriWhite(self.packet_bytes[offset], self.packet_bytes[offset + 1], self.packet_bytes[offset + 2]),
                })
            },
            ChannelType::Single => {
                Ok(ChannelValue {
                    channel: channel_definition.channel,
                    value: DimmerValue::Single(self.packet_bytes[offset]),
                })
            },
        }
    }

    pub fn send(&mut self) -> Result<(), ArtnetError> {
        if !self.disable_send {
            self.controller.send(self.packet_bytes.as_slice())?;
        }
        self.packet_bytes[DMX_SEQ_OFFSET] = self.packet_bytes[DMX_SEQ_OFFSET].wrapping_add(1);
        self.modified = false;
        self.non_modified_ticks = 0;
        Ok(())
    }
}