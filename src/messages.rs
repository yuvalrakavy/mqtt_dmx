use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::oneshot::Sender;
use crate::artnet_manager::EffectNodeRuntime;
use crate::defs::{self, EffectUsage};
use crate::{artnet_manager::ArtnetError, array_manager::DmxArrayError};

#[derive(Debug)]
pub enum ToArtnetManagerMessage {
    AddUniverse(Arc<str>, defs::UniverseDefinition, Sender<Result<(), ArtnetError>>),
    RemoveUniverse(Arc<str>, Sender<Result<(), ArtnetError>>),

    StartEffect(Arc<str>, Box<dyn EffectNodeRuntime>, Sender<Result<(), ArtnetError>>),
    StopEffect(Arc<str>, Sender<Result<(), ArtnetError>>),

    SetChannels(defs::SetChannelsParameters, Sender<Result<(), ArtnetError>>),
}

#[derive(Debug)]
pub enum ToMqttPublisherMessage {
    Error(String),
}

#[derive(Debug)]
pub enum ToArrayManagerMessage {
    AddArray(Arc<str>, Box<defs::DmxArray>, Sender<Result<(), DmxArrayError>>),
    RemoveArray(Arc<str>, Sender<Result<(), DmxArrayError>>),

    AddEffect(Arc<str>, defs::EffectNodeDefinition, Sender<Result<(), DmxArrayError>>),
    RemoveEffect(Arc<str>, Sender<Result<(), DmxArrayError>>),

    GetEffectRuntime(Arc<str>, EffectUsage, Option<Arc<str>>, Option<HashMap<String, String>>, usize, Sender<Result<Box<dyn EffectNodeRuntime>, DmxArrayError>>),

    AddValue(Arc<str>, Arc<str>, Sender<Result<(), DmxArrayError>>),
    RemoveValue(Arc<str>, Sender<Result<(), DmxArrayError>>),
}
