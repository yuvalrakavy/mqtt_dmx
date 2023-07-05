use std::{net::{IpAddr, UdpSocket}, collections::HashMap, iter::repeat, fmt::Debug, mem, sync::{Weak, Arc}};
use tokio::{select, sync::mpsc::Receiver};
use tokio_util::sync::CancellationToken;
use log::info;

use super::ArtnetError;
use crate::{defs::UniverseDefinition, dmx::*, messages::ToArtnetManagerMessage};

struct ArtnetController {
    socket: UdpSocket,
}

struct Universe {
    description: String,

    controller: Arc<ArtnetController>,
    packet_bytes: Vec<u8>,
}

pub trait EffectNodeRuntime : Debug + Send {
    fn tick(&mut self, artnet_manager: &mut ArtnetManager) -> Result<(), ArtnetError>;
    fn is_done(&self) -> bool;
}

pub struct ArtnetManager {
    universes: HashMap<String, Universe>,
    controllers: HashMap<IpAddr, Weak<ArtnetController>>,
    active_effects: HashMap<String, Box<dyn EffectNodeRuntime>>,
}

const DMX_DATA_OFFSET: usize = 18;
const DMX_SEQ_OFFSET: usize = 12;
const DMX_UDP_PORT: u16 = 0x1936;
const ARTNET_OPCODE_OUTPUT: u16 = 0x5000;

impl ArtnetManager {
    pub fn new() -> ArtnetManager {
        ArtnetManager {
            universes: HashMap::new(),
            controllers: HashMap::new(),
            active_effects: HashMap::new(),
        }
    }

    fn add_universe(&mut self, universe_id: &str, definition: UniverseDefinition) -> Result<(), ArtnetError> {
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

    fn remove_universe(&mut self, universe_id: &str) -> Result<(), ArtnetError> {
         self.universes.remove(universe_id).ok_or_else(|| ArtnetError::InvalidUniverse(universe_id.to_string()))?;

        let to_remove = self.controllers.iter().filter(|(_, c)| c.upgrade().is_none()).map(|(ip, _)| *ip).collect::<Vec<IpAddr>>();
        
        for ip in to_remove.iter() {
            self.controllers.remove(ip);
        }
    
        Ok(())
    }

    fn start_effect(&mut self, effect_id: &str, effect: Box<dyn EffectNodeRuntime>) -> Result<(), ArtnetError> {
        self.active_effects.insert(effect_id.to_owned(), effect);
        Ok(())
    }

    fn stop_effect(&mut self, effect_id: &str) -> Result<(), ArtnetError> {
        self.active_effects.remove(effect_id);
        Ok(())
    }

    fn tick(&mut self) -> Result<(), ArtnetError> {
        let mut active_effects = mem::take(&mut self.active_effects);

        for (_, effect) in active_effects.iter_mut() {
            effect.tick(self)?;
        }

        self.active_effects = active_effects;       // Move it back
        Ok(())
    }

    pub fn set_channel(&mut self, universe_id: &str, v: &ChannelValue) -> Result<(), ArtnetError> {
        match self.universes.get_mut(universe_id) {
            Some(u) => u.set_channel(v),
            None => Err(ArtnetError::InvalidUniverse(universe_id.to_string())),
        }
    }

    fn set_channels(&mut self, universe_id: &str, channel_value_list: &Vec<ChannelValue>) -> Result<(), ArtnetError> {
        for channel_value in channel_value_list {
            self.set_channel(universe_id, channel_value)?;
        }
        Ok(())
    }

    pub fn get_channel(&self, universe_id: &str, channel_definition: &ChannelDefinition) -> Result<ChannelValue, ArtnetError> {
        match self.universes.get(universe_id) {
            Some(u) => u.get_channel(channel_definition),
            None => Err(ArtnetError::InvalidUniverse(universe_id.to_string())),
        }
    }

    fn get_channels(&self, universe_id: &str, channel_definitions: &Vec<ChannelDefinition>) -> Result<Vec<ChannelValue>, ArtnetError> {
        let mut channel_values = Vec::new();

        for channel_definition in channel_definitions {
            channel_values.push(self.get_channel(universe_id, channel_definition)?);
        }
        Ok(channel_values)
    }

    fn send(&mut self, universe_id: &str) -> Result<(), ArtnetError> {
        match self.universes.get_mut(universe_id) {
            Some(u) => u.send(),
            None => Err(ArtnetError::InvalidUniverse(universe_id.to_string())),
        }
    }

    fn handle_message(&mut self, message: ToArtnetManagerMessage) {
        match message {
            ToArtnetManagerMessage::AddUniverse(universe_id, definition, reply_tx) => 
                reply_tx.send(self.add_universe(&universe_id, definition)).unwrap(),
            ToArtnetManagerMessage::RemoveUniverse(universe_id, sender) =>
                sender.send(self.remove_universe(&universe_id)).unwrap(),
            ToArtnetManagerMessage::SetChannel(universe_id, channel_value, reply_tx) =>
                reply_tx.send(self.set_channel(&universe_id, &channel_value)).unwrap(),
            ToArtnetManagerMessage::SetChannels(universe_id,  channel_value_list, reply_tx) =>
                reply_tx.send(self.set_channels(&universe_id, &channel_value_list)).unwrap(),
            ToArtnetManagerMessage::GetChannel(universe_id, channel_definition, reply_tx) =>
                reply_tx.send(self.get_channel(&universe_id, &channel_definition)).unwrap(),
            ToArtnetManagerMessage::GetChannels(universe_id, channel_definitions, reply_tx) =>
                reply_tx.send(self.get_channels(&universe_id, &channel_definitions)).unwrap(),
            ToArtnetManagerMessage::Send(universe_id, sender) =>
                sender.send(self.send(&universe_id)).unwrap(),
        }
    }

    pub async fn run(&mut self, cancel: CancellationToken, mut receiver: Receiver<ToArtnetManagerMessage>) {
        loop {
            select! {
                _ = cancel.cancelled() => break,

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
            packet_bytes,
        })     
    }

    #[cfg(test)]
    fn get_packet_bytes(&self) -> &Vec<u8> {
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
        self.controller.send(self.packet_bytes.as_slice())?;
        self.packet_bytes[DMX_SEQ_OFFSET] += 1;
        Ok(())
    }
}

#[cfg(test)]
mod test_universe {
    use std::str::FromStr;

    use super::*;

    fn get_universe_definition() -> UniverseDefinition {
        UniverseDefinition {
            description: "Test Universe".to_string(),
            controller: IpAddr::from_str("10.0.1.228").unwrap(),
            net: 0,
            subnet: 0,
            universe: 0,
            channels: 306,
        }
    }

    fn get_universe(universe_id: &str) -> Universe {
        let controller = Arc::new(ArtnetController::new(&IpAddr::from_str("10.0.1.228").unwrap()).unwrap());
        Universe::new(controller, universe_id, get_universe_definition()).unwrap()
    }

    #[test]
    fn test_universe_new() {
        let universe = get_universe("test");

        assert_eq!(universe.get_packet_bytes().len(), 306 + DMX_DATA_OFFSET);
    }

    #[test]
    fn test_set_channel() {
        let mut universe = get_universe("test");

        let channel_value = ChannelValue {
            channel: 0,
            value: DimmerValue::Single(255),
        };

        universe.set_channel(&channel_value).unwrap();
        let packet_bytes = universe.get_packet_bytes();
        assert_eq!(packet_bytes[DMX_DATA_OFFSET], 255);

        let channel_value = ChannelValue {
            channel: 305,
            value: DimmerValue::Single(255),
        };

        assert!(universe.set_channel(&channel_value).is_ok());
        let packet_bytes = universe.get_packet_bytes();
        assert_eq!(packet_bytes[DMX_DATA_OFFSET+305], 255);

        let channel_value = ChannelValue {
            channel: 0,
            value: DimmerValue::Rgb(10, 20, 30)
        };

        assert!(universe.set_channel(&channel_value).is_ok());
        let packet_bytes = universe.get_packet_bytes();
        assert_eq!(packet_bytes[DMX_DATA_OFFSET], 10);
        assert_eq!(packet_bytes[DMX_DATA_OFFSET+1], 20);
        assert_eq!(packet_bytes[DMX_DATA_OFFSET+2], 30);
    }

    #[test]
    fn test_get_channel() {
        let mut universe = get_universe("test");

        let channel_value = ChannelValue {
            channel: 0,
            value: DimmerValue::Single(255),
        };

        universe.set_channel(&channel_value).unwrap();
        let channel_value = universe.get_channel(&ChannelDefinition { channel: 0, channel_type: ChannelType::Single}).unwrap();
        assert_eq!(channel_value.value, DimmerValue::Single(255));
        assert_eq!(channel_value.channel, 0);

        let channel_value = ChannelValue {
            channel: 10,
            value: DimmerValue::TriWhite(19, 23, 30),
        };
        universe.set_channel(&channel_value).unwrap();
        let channel_value = universe.get_channel(&ChannelDefinition { channel: 10, channel_type: ChannelType::TriWhite}).unwrap();
        assert_eq!(channel_value.value, DimmerValue::TriWhite(19, 23, 30));
        assert_eq!(channel_value.channel, 10);
    }

    #[test]
    fn test_set_error_handling() {
        let mut universe = get_universe("test");

        let channel_value = ChannelValue {
            channel: 305,
            value: DimmerValue::Single(255),
        };
        assert!(universe.set_channel(&channel_value).is_ok());

        let channel_value = ChannelValue {
            channel: 305,
            value: DimmerValue::Rgb(10, 11, 12),
        };
        let result = universe.set_channel(&channel_value);

        match result {
            Err(ArtnetError::InvalidChannel(d, 305, 306)) if d == "test (Test Universe)" => {},
            _ => panic!("Expected InvalidChannel error, got {:?}", result),
        }
    }

    #[test]
    fn test_get_error_handling() {
        let universe = get_universe("test");

        assert!(universe.get_channel(&ChannelDefinition { channel:305, channel_type: ChannelType::Single}).is_ok());
        let result = universe.get_channel(&ChannelDefinition { channel: 305, channel_type: ChannelType::Rgb});

        match result {
            Err(ArtnetError::InvalidChannel(d, 305, 306)) if d == "test (Test Universe)" => {},
            _ => panic!("Expected InvalidChannel error, got {:?}", result),
        }
    }
}

#[cfg(test)]
mod test_dmx_manager {
    use super::*;
    use std::str::FromStr;
    use tokio::sync::mpsc::Sender;

    fn get_universe_definition() -> UniverseDefinition {
        UniverseDefinition {
            description: "Test Universe".to_string(),
            controller: IpAddr::from_str("10.0.1.228").unwrap(),
            net: 0,
            subnet: 0,
            universe: 0,
            channels: 306,
        }
    }

    fn start_dmx_manager(cancel: CancellationToken) -> Sender<ToArtnetManagerMessage> {
        let (sender, receiver) = tokio::sync::mpsc::channel::<ToArtnetManagerMessage>(10);

        tokio::spawn(async move {
            let mut manager = ArtnetManager::new();
            manager.run(cancel, receiver).await;
        });

        sender
    }

    #[test]
    fn test_add_universe() {
        let mut manager = ArtnetManager::new();

        let universe_definition = get_universe_definition();
        assert!(manager.add_universe("test", universe_definition).is_ok());
        assert!(manager.controllers.len() == 1);
        assert!(manager.controllers.len() == 1);
    }

    #[test]
    fn test_remove_universe() {
        let mut manager = ArtnetManager::new();

        let universe_definition = get_universe_definition();
        assert!(manager.add_universe("test1", universe_definition).is_ok());
        assert!(manager.controllers.len() == 1);
        assert!(manager.universes.len() == 1);

        let universe_definition = get_universe_definition();
        assert!(manager.add_universe("test2", universe_definition).is_ok());
        assert!(manager.controllers.len() == 1);
        assert!(manager.universes.len() == 2);

        // Remove universe and ensure that the controller is still there
        assert!(manager.remove_universe("test1").is_ok());
        assert!(manager.universes.len() == 1);
        assert!(manager.controllers.len() == 1);

        // Remove the second universe and ensure that the controller is gone
        assert!(manager.remove_universe("test2").is_ok());
        assert!(manager.universes.len() == 0);
        assert!(manager.controllers.len() == 0);
    }

    #[test]
    fn test_universe_set() {
        let mut manager = ArtnetManager::new();

        let universe_definition = get_universe_definition();
        assert!(manager.add_universe("test", universe_definition).is_ok());

        let channel_value = ChannelValue {
            channel: 5,
            value: DimmerValue::Single(255),
        };

        assert!(manager.set_channel("test", &channel_value).is_ok());
        let v = manager.get_channel("test", &ChannelDefinition { channel: 5, channel_type: ChannelType::Single}).unwrap();
        assert_eq!(v.value, DimmerValue::Single(255));

        let channel_value = ChannelValue {
            channel: 10,
            value: DimmerValue::Rgb(3, 5, 8)
        };

        assert!(manager.set_channel("test", &channel_value).is_ok());
        let v = manager.get_channel("test", &ChannelDefinition { channel: 10, channel_type: ChannelType::Rgb}).unwrap();
        assert_eq!(v.value, DimmerValue::Rgb(3, 5, 8));
    }

    #[tokio::test]
    async fn test_messaging() {
        let cancel = CancellationToken::new();
        let sender = start_dmx_manager(cancel.clone());
        let universe_definition = get_universe_definition();
        let (tx, rx) = tokio::sync::oneshot::channel();

        sender.send(ToArtnetManagerMessage::AddUniverse("test".to_string(), universe_definition, tx)).await.unwrap();
        let result = rx.await.unwrap();
        cancel.cancel();
        assert!(result.is_ok());
    }
}