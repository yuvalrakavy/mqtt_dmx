use std::sync::Arc;

use tokio::sync::oneshot::Sender;
use crate::artnet_manager::EffectNodeRuntime;
use crate::defs::{self, EffectUsage, SymbolTable};
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

    GetEffectRuntime(Arc<str>, EffectUsage, Option<Arc<str>>, usize, Sender<Result<Box<dyn EffectNodeRuntime>, DmxArrayError>>),

    InitializeArrayValues(Arc<str>, SymbolTable, Sender<Result<(), DmxArrayError>>),
    AddGlobalValue(Arc<str>, Arc<str>, Sender<Result<(), DmxArrayError>>),
    RemoveGlobalValue(Arc<str>, Sender<Result<(), DmxArrayError>>),
}
