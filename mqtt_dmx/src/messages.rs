use std::sync::Arc;
use error_stack::Report;

use tokio::sync::oneshot::Sender;
use crate::artnet_manager::EffectNodeRuntime;
use crate::defs::{self, EffectUsage, SymbolTable};
use crate::{artnet_manager::ArtnetError, array_manager::DmxArrayError};

#[derive(Debug)]
pub enum ToArtnetManagerMessage {
    AddUniverse(Arc<str>, defs::UniverseDefinition, Sender<Result<(), Report<ArtnetError>>>),
    RemoveUniverse(Arc<str>, Sender<Result<(), Report<ArtnetError>>>),

    StartEffect(Arc<str>, Box<dyn EffectNodeRuntime>, Sender<Result<(), Report<ArtnetError>>>),
    StopEffect(Arc<str>, Sender<Result<(), Report<ArtnetError>>>),

    SetChannels(defs::SetChannelsParameters, Sender<Result<(), Report<ArtnetError>>>),
}

#[derive(Debug)]
pub enum ToMqttPublisherMessage {
    Error(String),
}

#[derive(Debug)]
pub enum ToArrayManagerMessage {
    AddArray(Arc<str>, Box<defs::DmxArray>, Sender<Result<(), Report<DmxArrayError>>>),
    RemoveArray(Arc<str>, Sender<Result<(), Report<DmxArrayError>>>),

    AddEffect(Arc<str>, defs::EffectNodeDefinition, Sender<Result<(), Report<DmxArrayError>>>),
    RemoveEffect(Arc<str>, Sender<Result<(), Report<DmxArrayError>>>),

    GetEffectRuntime(Arc<str>, EffectUsage, Option<Arc<str>>, usize, Sender<Result<Box<dyn EffectNodeRuntime>, Report<DmxArrayError>>>),

    InitializeArrayValues(Arc<str>, SymbolTable, Sender<Result<(), Report<DmxArrayError>>>),
    AddGlobalValue(Arc<str>, Arc<str>, Sender<Result<(), Report<DmxArrayError>>>),
    RemoveGlobalValue(Arc<str>, Sender<Result<(), Report<DmxArrayError>>>),
}
