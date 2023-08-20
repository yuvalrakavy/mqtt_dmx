
use std::sync::Arc;

use thiserror::Error;
use super::verify::ChannelUsage;

#[derive(Debug, Error)]
pub enum DmxArrayError {
    #[error("Array with id '{0}' not found")]
    ArrayNotFound(Arc<str>),

    #[error("Array '{0}' Lights {1} does not contain definition for {2}")]
    ArrayLightsNotFound(String, String, String),

    #[error("Array '{0}' Light '{1}' ({2}) contain circular reference to {3}")]
    ArrayLightsCircularReference(String, String, String, String),

    #[error("Array '{0}' Light '{1}' ({2}) is invalid channel definition (s:n, rgb:n or w:n)")]
    ArrayLightsInvalidChannelDefinition(String, String, String),

    #[error("Effect '{0}' not found in array '{1}' or in global effects list")]
    EffectNotFound(Arc<str>, Arc<str>),

    #[error("Array '{0}' '{1}' has no value for {2}")]
    ArrayValueNotFound(Arc<str>, String, String),

    #[error("Array '{0}' '{1}' has unterminated `value` expression")]
    ValueExpressionNotTerminated(Arc<str>, Arc<str>),

    #[error("Array '{0}' in universe '{1}': channel {2} was defined as {3} and is redefined as {4} in group @{5}")]
    ArrayLightChannelUsageMismatch(String, String, u16, ChannelUsage, ChannelUsage, String),

    #[error("Array '{0}' in universe '{1}': channel {2} is defined as {3} in group @{4} but is not included in @all group")]
    ArrayLightChannelNotInAllGroup(String, String, u16, ChannelUsage, String),

    #[error("{0} {1}: {2}")]
    ValueError(String, &'static str, String),
}
