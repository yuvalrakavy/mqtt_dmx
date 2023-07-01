use std::str::FromStr;
use thiserror::Error;
use crate::defs::TargetValue;

#[derive(Debug, Error)]
pub enum DmxError {
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
}

#[derive(Debug, PartialEq, Eq)]
pub enum ChannelType {
    Rgb,
    TriWhite,
    Single,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ChannelDefinition {
    pub channel: u16,
    pub channel_type: ChannelType,
}

#[derive(Debug)]
pub struct UniverseChannelDefinitions {
    pub universe_id: String,
    pub channels: Vec<ChannelDefinition>, 
}

#[derive(Debug, PartialEq, Eq)]
pub enum DimmerValue {
    Rgb (u8, u8, u8),
    TriWhite (u8, u8, u8),
    Single (u8),
}

#[derive(Debug, PartialEq, Eq)]
pub struct ChannelValue {
    pub channel: u16,
    pub value: DimmerValue,
}

impl FromStr for DimmerValue {
    type Err = DmxError;

    /// Parse a string into a DimmerValue
    /// 
    /// String syntax
    /// s(n) -> DimmerValue::Single(n)
    /// rgb(r,g,b) -> DimmerValue::Rgb(r,g,b)
    /// w(w1, w2, w3) -> DimmerValue::TriWhite(w1, w2, w3)
    /// 
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let open_parenthesis = s.find('(').ok_or_else(|| DmxError::InvalidDimmerValue(s.to_string()))?;
        let close_parenthesis = s.find(')').ok_or_else(|| DmxError::InvalidDimmerValue(s.to_string()))?;
        let value_type = s[..open_parenthesis].trim();
        let values = s[open_parenthesis+1..close_parenthesis].split(',').map(|v| v.trim().parse::<u8>()).collect::<Result<Vec<u8>, _>>().map_err(|_| DmxError::InvalidDimmerValue(s.to_string()))?;

        match value_type.to_lowercase().as_str() {
            "s" if values.len() == 1 => Ok(DimmerValue::Single(values[0])),
            "rgb" if values.len() == 3 => Ok(DimmerValue::Rgb(values[0], values[1], values[2])),
            "w" if values.len() == 3 => Ok(DimmerValue::TriWhite(values[0], values[1], values[2])),
            _ => Err(DmxError::InvalidDimmerValue(s.to_string())),
        }
    }
}

impl FromStr for ChannelDefinition {
    type Err = DmxError;

    /// Parse a string into a ChannelDefinition
    /// 
    /// string syntax
    /// n -> ChannelDefinition { channel: n, channel_type: ChannelType::Single }
    /// s:n -> ChannelDefinition { channel: n, channel_type: ChannelType::Single }
    /// rgb:n -> ChannelDefinition { channel: n, channel_type: ChannelType::RGB }
    /// w:n -> ChannelDefinition { channel: n, channel_type: ChannelType::TriWhite }
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let column = s.find(':');

        let (channel_type, channel) = match column {
            Some(c) => (s[..c].trim(), s[c+1..].trim()),
            None => ("s", s.trim()),
        };

        let channel = channel.parse::<u16>().map_err(|_| DmxError::InvalidChannelAddress(s.to_string()))?;

        match channel_type.to_lowercase().as_str() {
            "rgb" => Ok(ChannelDefinition { channel, channel_type: ChannelType::Rgb }),
            "w" => Ok(ChannelDefinition { channel, channel_type: ChannelType::TriWhite }),
            "s" => Ok(ChannelDefinition { channel, channel_type: ChannelType::Single }),
            _ => Err(DmxError::InvalidChannelAddress(s.to_string())),
        }
    }
}

impl TargetValue {
    pub fn get(&self, channel_type: ChannelType) -> Option<DimmerValue> {
        match channel_type {
            ChannelType::Rgb => self.rgb.map(|(r,g,b)| DimmerValue::Rgb(r,g,b)),
            ChannelType::TriWhite => self.tri_white.map(|(w1,w2,w3)| DimmerValue::TriWhite(w1,w2,w3)),
            ChannelType::Single => self.single.map(DimmerValue::Single),
        }
    }
}

impl FromStr for TargetValue {
    type Err = DmxError;

    /// Parse a string into a TargetValue
    /// 
    /// string syntax:
    ///  [s(n)];[rgb(r,g,b)];[w(w1,w2,w3)]
    /// 
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let values = s.split(';').map(|v| v.trim().parse::<DimmerValue>()).collect::<Result<Vec<DimmerValue>, _>>()?;

        let mut target_value = TargetValue::default();
        for value in values {
            match value {
                DimmerValue::Single(s) => 
                    target_value.single = target_value.single.map_or(Ok(Some(s)), |_| Err(DmxError::AmbiguousTargetValue(s.to_string())))?,
                DimmerValue::Rgb(r,g,b) => 
                    target_value.rgb = target_value.rgb.map_or(Ok(Some((r, g, b))), |_| Err(DmxError::AmbiguousTargetValue(s.to_string())))?,
                DimmerValue::TriWhite(w1,w2,w3) => 
                    target_value.tri_white = target_value.tri_white.map_or(Ok(Some((w1, w2, w3))), |_| Err(DmxError::AmbiguousTargetValue(s.to_string())))?
            };
        }

        Ok(target_value)
    }
}


#[cfg(test)]
mod test_parse_value {
    use super::*;

    #[test]
    fn test_dimmer_value() {
        let v = "s(10)".parse::<DimmerValue>().unwrap();
        assert_eq!(v, DimmerValue::Single(10));

        let v = "rgb(10, 20, 30)".parse::<DimmerValue>().unwrap();
        assert_eq!(v, DimmerValue::Rgb(10, 20, 30));

        let v = "w(5,6, 7)".parse::<DimmerValue>().unwrap();
        assert_eq!(v, DimmerValue::TriWhite(5, 6, 7));
    }

    #[test]
    fn test_channel_definition() {
        let v = "rgb:1".parse::<ChannelDefinition>().unwrap();
        assert_eq!(v, ChannelDefinition {
            channel: 1,
            channel_type: ChannelType::Rgb,
        });

        let v = "s:2".parse::<ChannelDefinition>().unwrap();
        assert_eq!(v, ChannelDefinition {
            channel: 2,
            channel_type: ChannelType::Single,
        });

        let v = "2".parse::<ChannelDefinition>().unwrap();
        assert_eq!(v, ChannelDefinition {
            channel: 2,
            channel_type: ChannelType::Single,
        });

        let v = "w:3".parse::<ChannelDefinition>().unwrap();
        assert_eq!(v, ChannelDefinition {
            channel: 3,
            channel_type: ChannelType::TriWhite,
        });
    }

    #[test]
    fn test_target_value() {
        let v = "s(10);rgb(10,20,30);w(5,6,7)".parse::<TargetValue>().unwrap();

        assert_eq!(v.get(ChannelType::Single), Some(DimmerValue::Single(10)));
        assert_eq!(v.get(ChannelType::Rgb), Some(DimmerValue::Rgb(10, 20, 30)));
        assert_eq!(v.get(ChannelType::TriWhite), Some(DimmerValue::TriWhite(5, 6, 7)));

        let v = "s(10)".parse::<TargetValue>().unwrap();
        assert_eq!(v.get(ChannelType::Single), Some(DimmerValue::Single(10)));
        assert_eq!(v.get(ChannelType::Rgb), None);
        assert_eq!(v.get(ChannelType::TriWhite), None);

        let v = "rgb(10,20,30)".parse::<TargetValue>().unwrap();
        assert_eq!(v.get(ChannelType::Single), None);
        assert_eq!(v.get(ChannelType::Rgb), Some(DimmerValue::Rgb(10, 20, 30)));
        assert_eq!(v.get(ChannelType::TriWhite), None);

        let v = "w(5,6,7)".parse::<TargetValue>().unwrap();
        assert_eq!(v.get(ChannelType::Single), None);
        assert_eq!(v.get(ChannelType::Rgb), None);
        assert_eq!(v.get(ChannelType::TriWhite), Some(DimmerValue::TriWhite(5, 6, 7)));

        let v = "s(10);s(20)".parse::<TargetValue>();

        if let Err(DmxError::AmbiguousTargetValue(_)) = v {
        } else {
            panic!("Expected AmbiguousTargetValue error");
        }
    }
}

