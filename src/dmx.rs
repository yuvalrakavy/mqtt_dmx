use crate::artnet_manager::ArtnetError;
use crate::defs::{DimmingAmount, TargetValue};
use std::fmt::{Display, Formatter};
use std::str::FromStr;

// #[derive(Debug, PartialEq, Eq, Copy, Clone)]
// pub enum ChannelType {
//     Rgb,
//     TriWhite,
//     Single,
// }

// #[derive(Debug, PartialEq, Eq)]
// pub struct ChannelDefinition {
//     pub channel: u16,
//     pub channel_type: ChannelType,
// }

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum ChannelDefinition {
    Single(u16),
    Rgb(u16, u16, u16),
    TriWhite(u16, u16, u16),
}

impl FromStr for ChannelDefinition {
    type Err = ArtnetError;

    /// Parse a string into a ChannelDefinition
    ///
    /// string syntax
    /// n -> ChannelDefinition { channel: n, channel_type: ChannelType::Single }
    /// s:n -> ChannelDefinition { channel: n, channel_type: ChannelType::Single }
    /// rgb:n -> ChannelDefinition { channel: n, channel_type: ChannelType::RGB }
    /// w:n -> ChannelDefinition { channel: n, channel_type: ChannelType::TriWhite }
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        fn is_diff(c1: u16, c2: u16, c3: u16) -> std::result::Result<(), ArtnetError> {
            if c1 == c2 || c1 == c3 || c2 == c3 {
                return Err(ArtnetError::InvalidChannelAddress(format!(
                    "rgb or w individual channel addresses must be different: {}, {}, {}",
                    c1, c2, c3
                )));
            }
            Ok(())
        }
        let column = s.find(':');

        let (channel_type, channel) = match column {
            Some(c) => (s[..c].trim(), s[c + 1..].trim()),
            None => ("s", s.trim()),
        };

        let channels: Vec<u16> = channel
            .split('/')
            .map(|c| {
                c.trim()
                    .parse::<u16>()
                    .map_err(|_| ArtnetError::InvalidChannelAddress(s.to_string()))
            })
            .collect::<std::result::Result<Vec<u16>, ArtnetError>>()?;

        if channels.is_empty() {
            return Err(ArtnetError::InvalidChannelAddress(s.to_string()));
        }

        Ok(match channel_type.to_lowercase().as_str() {
            "rgb" => match channels.len() {
                1 => ChannelDefinition::Rgb(channels[0], channels[0]+1, channels[0]+2),
                3 => {
                    is_diff(channels[0], channels[1], channels[2])?;
                    ChannelDefinition::Rgb(channels[0], channels[1], channels[2])
                }
                _ => return Err(ArtnetError::InvalidChannelAddress(s.to_string())),
            },
            "w" => match channels.len() {
                1 => ChannelDefinition::TriWhite(channels[0], channels[0]+1, channels[0]+2),
                3 => {
                    is_diff(channels[0], channels[1], channels[2])?;
                    ChannelDefinition::TriWhite(channels[0], channels[1], channels[2])
                }
                _ => return Err(ArtnetError::InvalidChannelAddress(s.to_string())),
            },
            "s" => match channels.len() {
                1 => ChannelDefinition::Single(channels[0]),
                _ => return Err(ArtnetError::InvalidChannelAddress(s.to_string())),
            },
            _ => return Err(ArtnetError::InvalidChannelAddress(s.to_string())),
        })
    }
}

impl Display for ChannelDefinition {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ChannelDefinition::Rgb(r, g, b) => write!(f, "rgb:{}/{}/{}", r, g, b),
            ChannelDefinition::TriWhite(w1, w2, w3) => write!(f, "w:{}/{}/{}", w1, w2, w3),
            ChannelDefinition::Single(c) => write!(f, "s({})", c),
        }
    }
}

#[derive(Debug)]
pub struct UniverseChannelDefinitions {
    pub universe_id: String,
    pub channels: Vec<ChannelDefinition>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum DimmerValue {
    Rgb(u8, u8, u8),
    TriWhite(u8, u8, u8),
    Single(u8),
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ChannelValue {
    pub channel: ChannelDefinition,
    pub value: DimmerValue,
}

impl FromStr for DimmerValue {
    type Err = ArtnetError;

    /// Parse a string into a DimmerValue
    ///
    /// String syntax
    /// s(n) -> DimmerValue::Single(n)
    /// rgb(r,g,b) -> DimmerValue::Rgb(r,g,b)
    /// w(w1, w2, w3) -> DimmerValue::TriWhite(w1, w2, w3)
    ///
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let open_parenthesis = s
            .find('(')
            .ok_or_else(|| ArtnetError::InvalidDimmerValue(s.to_string()))?;
        let close_parenthesis = s
            .find(')')
            .ok_or_else(|| ArtnetError::InvalidDimmerValue(s.to_string()))?;
        let value_type = s[..open_parenthesis].trim();
        let values = s[open_parenthesis + 1..close_parenthesis]
            .split(',')
            .map(|v| v.trim().parse::<u8>())
            .collect::<std::result::Result<Vec<u8>, _>>()
            .map_err(|_| ArtnetError::InvalidDimmerValue(s.to_string()))?;

        match value_type.to_lowercase().as_str() {
            "s" if values.len() == 1 => Ok(DimmerValue::Single(values[0])),
            "rgb" if values.len() == 3 => Ok(DimmerValue::Rgb(values[0], values[1], values[2])),
            "w" if values.len() == 3 => Ok(DimmerValue::TriWhite(values[0], values[1], values[2])),
            _ => Err(ArtnetError::InvalidDimmerValue(s.to_string())),
        }
    }
}

impl Display for DimmerValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match *self {
            DimmerValue::Rgb(r, g, b) => write!(f, "rgb({},{},{})", r, g, b),
            DimmerValue::TriWhite(w1, w2, w3) => write!(f, "w({},{},{})", w1, w2, w3),
            DimmerValue::Single(v) => write!(f, "s({})", v)
        }
    }
}

impl TargetValue {
    pub fn get(&self, channel_definition: &ChannelDefinition) -> Option<DimmerValue> {
        match channel_definition {
            ChannelDefinition::Rgb(_, _, _) => self.rgb.map(|(r, g, b)| DimmerValue::Rgb(r, g, b)),
            ChannelDefinition::TriWhite(_, _, _) => self
                .tri_white
                .map(|(w1, w2, w3)| DimmerValue::TriWhite(w1, w2, w3)),
            ChannelDefinition::Single(_) => self.single.map(DimmerValue::Single),
        }
    }

    pub fn get_dimmed_value(&self, dimming_amount: DimmingAmount) -> TargetValue {
        TargetValue {
            rgb: self.rgb.map(|(r, g, b)| {
                (
                    (r as DimmingAmount * dimming_amount / 1000) as u8,
                    (g as DimmingAmount * dimming_amount / 1000) as u8,
                    (b as DimmingAmount * dimming_amount / 1000) as u8,
                )
            }),
            tri_white: self.tri_white.map(|(w1, w2, w3)| {
                (
                    (w1 as DimmingAmount * dimming_amount / 1000) as u8,
                    (w2 as DimmingAmount * dimming_amount / 1000) as u8,
                    (w3 as DimmingAmount * dimming_amount / 1000) as u8,
                )
            }),
            single: self
                .single
                .map(|v| (v as DimmingAmount * dimming_amount / 1000) as u8),
        }
    }
}

impl FromStr for TargetValue {
    type Err = ArtnetError;

    /// Parse a string into a TargetValue
    ///
    /// string syntax:
    ///  [s(n)];[rgb(r,g,b)];[w(w1,w2,w3)]
    ///
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let values = s
            .split(';')
            .map(|v| v.trim().parse::<DimmerValue>())
            .collect::<std::result::Result<Vec<DimmerValue>, _>>()?;

        let mut target_value = TargetValue::default();
        for value in values {
            match value {
                DimmerValue::Single(s) => {
                    target_value.single = target_value.single.map_or(Ok(Some(s)), |_| {
                        Err(ArtnetError::AmbiguousTargetValue(s.to_string()))
                    })?
                }
                DimmerValue::Rgb(r, g, b) => {
                    target_value.rgb = target_value.rgb.map_or(Ok(Some((r, g, b))), |_| {
                        Err(ArtnetError::AmbiguousTargetValue(s.to_string()))
                    })?
                }
                DimmerValue::TriWhite(w1, w2, w3) => {
                    target_value.tri_white =
                        target_value.tri_white.map_or(Ok(Some((w1, w2, w3))), |_| {
                            Err(ArtnetError::AmbiguousTargetValue(s.to_string()))
                        })?
                }
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
        assert_eq!(v, ChannelDefinition::Rgb(1, 2, 3));

        let v = "rgb:1/3/5".parse::<ChannelDefinition>().unwrap();
        assert_eq!(v, ChannelDefinition::Rgb(1, 3, 5));

        let v = "s:2".parse::<ChannelDefinition>().unwrap();
        assert_eq!(v, ChannelDefinition::Single(2));

        let v = "2".parse::<ChannelDefinition>().unwrap();
        assert_eq!(v, ChannelDefinition::Single(2));

        let v = "w:3".parse::<ChannelDefinition>().unwrap();
        assert_eq!(v, ChannelDefinition::TriWhite(3, 4, 5));

        let v = "w:3/10/400".parse::<ChannelDefinition>().unwrap();
        assert_eq!(v, ChannelDefinition::TriWhite(3, 10, 400));
    }

    #[test]
    fn test_target_value() {
        let v = "s(10);rgb(10,20,30);w(5,6,7)"
            .parse::<TargetValue>()
            .unwrap();

        assert_eq!(
            v.get(&ChannelDefinition::Single(1)),
            Some(DimmerValue::Single(10))
        );
        assert_eq!(
            v.get(&ChannelDefinition::Rgb(1, 2, 3)),
            Some(DimmerValue::Rgb(10, 20, 30))
        );
        assert_eq!(
            v.get(&ChannelDefinition::TriWhite(1, 2, 3)),
            Some(DimmerValue::TriWhite(5, 6, 7))
        );

        let v = "s(10)".parse::<TargetValue>().unwrap();
        assert_eq!(v.get(&ChannelDefinition::Single(10)), Some(DimmerValue::Single(10)));
        assert_eq!(v.get(&ChannelDefinition::Rgb(5,4,2)), None);
        assert_eq!(v.get(&ChannelDefinition::TriWhite(1,2,3)), None);

        let v = "rgb(10,20,30)".parse::<TargetValue>().unwrap();
        assert_eq!(v.get(&ChannelDefinition::Single(1)), None);
        assert_eq!(v.get(&ChannelDefinition::Rgb(1,3,4)), Some(DimmerValue::Rgb(10, 20, 30)));
        assert_eq!(v.get(&ChannelDefinition::TriWhite(2,3,4)), None);

        let v = "w(5,6,7)".parse::<TargetValue>().unwrap();
        assert_eq!(v.get(&ChannelDefinition::Single(1)), None);
        assert_eq!(v.get(&ChannelDefinition::Rgb(5,4,2)), None);
        assert_eq!(
            v.get(&ChannelDefinition::TriWhite(1,2,3)),
            Some(DimmerValue::TriWhite(5, 6, 7))
        );

        let v = "s(10);s(20)".parse::<TargetValue>();

        if let Err(ArtnetError::AmbiguousTargetValue(_)) = v {
        } else {
            panic!("Expected AmbiguousTargetValue error");
        }
    }
}
