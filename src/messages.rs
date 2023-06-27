use tokio::sync::oneshot::Sender;
use crate::defs;
use crate::{dmx, dmx::DmxError, array_manager::DmxArrayError};


#[derive(Debug)]
pub enum ToArtnetManagerMessage {
    AddUniverse(String, defs::UniverseDefinition, Sender<Result<(), DmxError>>),
    RemoveUniverse(String, Sender<Result<(), DmxError>>),

    SetChannel(String, dmx::ChannelValue, Sender<Result<(), DmxError>>),
    GetChannel(String, dmx::ChannelDefinition, Sender<Result<dmx::ChannelValue, DmxError>>),
    SetChannels(String, Vec<dmx::ChannelValue>, Sender<Result<(), DmxError>>),
    GetChannels(String, Vec<dmx::ChannelDefinition>, Sender<Result<Vec<dmx::ChannelValue>, DmxError>>),

    Send(String, Sender<Result<(), DmxError>>),
}

#[derive(Debug)]
pub enum ToMqttPublisherMessage {
    Error(String),
}

#[derive(Debug)]
pub enum ToArrayManagerMessage {
    AddArray(String, defs::DmxArray, Sender<Result<(), DmxArrayError>>),
    RemoveArray(String, Sender<Result<(), DmxArrayError>>),

    GetLightChannels(String, String, Sender<Result<Vec<dmx::UniverseChannelDefinitions>, DmxArrayError>>),
}
