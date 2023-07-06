use std::collections::HashMap;

use tokio::sync::oneshot::Sender;
use crate::artnet_manager::EffectNodeRuntime;
use crate::defs::{self, EffectUsage};
use crate::{dmx, artnet_manager::ArtnetError, array_manager::DmxArrayError};


#[derive(Debug)]
pub enum ToArtnetManagerMessage {
    AddUniverse(String, defs::UniverseDefinition, Sender<Result<(), ArtnetError>>),
    RemoveUniverse(String, Sender<Result<(), ArtnetError>>),

    StartEffect(String, Box<dyn EffectNodeRuntime>, Sender<Result<(), ArtnetError>>),

    SetChannel(String, dmx::ChannelValue, Sender<Result<(), ArtnetError>>),
    GetChannel(String, dmx::ChannelDefinition, Sender<Result<dmx::ChannelValue, ArtnetError>>),
    SetChannels(String, Vec<dmx::ChannelValue>, Sender<Result<(), ArtnetError>>),
    GetChannels(String, Vec<dmx::ChannelDefinition>, Sender<Result<Vec<dmx::ChannelValue>, ArtnetError>>),
}

#[derive(Debug)]
pub enum ToMqttPublisherMessage {
    Error(String),
}

#[derive(Debug)]
pub enum ToArrayManagerMessage {
    AddArray(String, defs::DmxArray, Sender<Result<(), DmxArrayError>>),
    RemoveArray(String, Sender<Result<(), DmxArrayError>>),
    GetEffectRuntime(String, EffectUsage, Option<usize>, Option<HashMap<String, String>>, usize, Sender<Result<Box<dyn EffectNodeRuntime>, DmxArrayError>>),

    AddValues(HashMap<String, String>, Sender<Result<(), DmxArrayError>>),
    RemoveValues(Sender<Result<(), DmxArrayError>>),
}
