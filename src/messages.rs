use std::collections::HashMap;

use tokio::sync::oneshot::Sender;
use crate::artnet_manager::EffectNodeRuntime;
use crate::defs::{self, EffectUsage};
use crate::{artnet_manager::ArtnetError, array_manager::DmxArrayError};


#[derive(Debug)]
pub enum ToArtnetManagerMessage {
    AddUniverse(String, defs::UniverseDefinition, Sender<Result<(), ArtnetError>>),
    RemoveUniverse(String, Sender<Result<(), ArtnetError>>),

    StartEffect(String, Box<dyn EffectNodeRuntime>, Sender<Result<(), ArtnetError>>),
    StopEffect(String, Sender<Result<(), ArtnetError>>),
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
