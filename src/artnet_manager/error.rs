
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ArtnetError {
    #[error("Invalid universe number: {0} (must be less than 16)")]
    InvalidUniverseNumber(u8),

    #[error("No universe with ID '{0}' is defined")]
    InvalidUniverse(String),

    #[error("Invalid subnet number: {0} (must be less than 16)")]
    InvalidSubnet(u8),

    #[error("Invalid net number: {0} (must be less than 128)")]
    InvalidNet(u8),

    #[error("Too many channels: {0} (must be less than 512)")]
    TooManyChannels(u16),

    #[error("Invalid channel address for universe {0}: {1} (must be less than {2})")]
    InvalidChannel(String, u16, u16),

    #[error("Invalid channel address: '{0}")]
    InvalidChannelAddress(String),

    #[error("Connection error")]
    ConnectionError(#[from] std::io::Error),

    #[error("Invalid dimmer value: '{0}'")]
    InvalidDimmerValue(String),

    #[error("Ambiguous target value: '{0}'")]
    AmbiguousTargetValue(String),

    #[error("You try to set a value of channel {0} however target {1} has no value for this type of channel")]
    MissingTargetValue(String, String),
}
