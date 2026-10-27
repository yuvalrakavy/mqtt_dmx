//! Count decoded JSON admission units without constructing a JSON value.
use serde::ser::{self, Serialize};
use std::fmt;

type Result<T> = std::result::Result<T, serde_json::Error>;

pub(super) struct Meter {
    used: usize,
    limit: usize,
}

impl Meter {
    fn charge(&mut self, units: usize) -> Result<()> {
        self.used = self.used.saturating_add(units);
        if self.used > self.limit {
            return Err(ser::Error::custom("16MiB admission budget exceeded"));
        }
        Ok(())
    }
}

pub(super) fn measure(value: &impl Serialize, limit: usize) -> Result<usize> {
    let mut meter = Meter { used: 0, limit };
    value.serialize(&mut meter)?;
    Ok(meter.used)
}

pub(super) struct Compound<'a> {
    meter: &'a mut Meter,
}

macro_rules! scalar {
    ($($method:ident($ty:ty)),* $(,)?) => {$(
        fn $method(self, _: $ty) -> Result<()> { self.charge(32) }
    )*};
}

impl<'a> ser::Serializer for &'a mut Meter {
    type Ok = ();
    type Error = serde_json::Error;
    type SerializeSeq = Compound<'a>;
    type SerializeTuple = Compound<'a>;
    type SerializeTupleStruct = Compound<'a>;
    type SerializeTupleVariant = Compound<'a>;
    type SerializeMap = Compound<'a>;
    type SerializeStruct = Compound<'a>;
    type SerializeStructVariant = Compound<'a>;

    scalar!(
        serialize_bool(bool),
        serialize_i8(i8),
        serialize_i16(i16),
        serialize_i32(i32),
        serialize_i64(i64),
        serialize_i128(i128),
        serialize_u8(u8),
        serialize_u16(u16),
        serialize_u32(u32),
        serialize_u64(u64),
        serialize_u128(u128),
        serialize_f32(f32),
        serialize_f64(f64)
    );

    fn serialize_char(self, value: char) -> Result<()> {
        self.charge(32 + value.len_utf8())
    }
    fn serialize_str(self, value: &str) -> Result<()> {
        self.charge(32usize.saturating_add(value.len()))
    }
    fn serialize_bytes(self, value: &[u8]) -> Result<()> {
        self.charge(64usize.saturating_add(value.len().saturating_mul(96)))
    }
    fn serialize_none(self) -> Result<()> {
        self.charge(32)
    }
    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<()> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<()> {
        self.charge(32)
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<()> {
        self.charge(32)
    }
    fn serialize_unit_variant(self, _: &'static str, _: u32, variant: &'static str) -> Result<()> {
        self.serialize_str(variant)
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _: &'static str,
        value: &T,
    ) -> Result<()> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<()> {
        self.charge(128usize.saturating_add(variant.len()))?;
        value.serialize(self)
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Compound<'a>> {
        self.charge(64)?;
        Ok(Compound { meter: self })
    }
    fn serialize_tuple(self, len: usize) -> Result<Compound<'a>> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_struct(self, _: &'static str, len: usize) -> Result<Compound<'a>> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Compound<'a>> {
        self.charge(128usize.saturating_add(variant.len()))?;
        self.serialize_seq(Some(len))
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Compound<'a>> {
        self.charge(64)?;
        Ok(Compound { meter: self })
    }
    fn serialize_struct(self, _: &'static str, len: usize) -> Result<Compound<'a>> {
        self.serialize_map(Some(len))
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Compound<'a>> {
        self.charge(128usize.saturating_add(variant.len()))?;
        self.serialize_map(Some(len))
    }
    fn collect_str<T: ?Sized + fmt::Display>(self, value: &T) -> Result<()> {
        struct Count<'a>(&'a mut Meter);
        impl fmt::Write for Count<'_> {
            fn write_str(&mut self, value: &str) -> fmt::Result {
                self.0.charge(value.len()).map_err(|_| fmt::Error)
            }
        }
        self.charge(32)?;
        fmt::write(&mut Count(self), format_args!("{value}"))
            .map_err(|_| ser::Error::custom("16MiB admission budget exceeded"))
    }
}

macro_rules! sequence {
    ($trait:ident, $method:ident) => {
        impl ser::$trait for Compound<'_> {
            type Ok = ();
            type Error = serde_json::Error;
            fn $method<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
                self.meter.charge(64)?;
                value.serialize(&mut *self.meter)
            }
            fn end(self) -> Result<()> {
                Ok(())
            }
        }
    };
}
sequence!(SerializeSeq, serialize_element);
sequence!(SerializeTuple, serialize_element);
sequence!(SerializeTupleStruct, serialize_field);
sequence!(SerializeTupleVariant, serialize_field);

impl ser::SerializeMap for Compound<'_> {
    type Ok = ();
    type Error = serde_json::Error;
    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<()> {
        // Serializing the key through this meter overcounts its scalar header;
        // it never allocates an intermediate key or whole representation.
        self.meter.charge(64)?;
        key.serialize(&mut *self.meter)
    }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
        value.serialize(&mut *self.meter)
    }
    fn end(self) -> Result<()> {
        Ok(())
    }
}
macro_rules! structure {
    ($trait:ident) => {
        impl ser::$trait for Compound<'_> {
            type Ok = ();
            type Error = serde_json::Error;
            fn serialize_field<T: ?Sized + Serialize>(
                &mut self,
                key: &'static str,
                value: &T,
            ) -> Result<()> {
                self.meter.charge(64usize.saturating_add(key.len()))?;
                value.serialize(&mut *self.meter)
            }
            fn end(self) -> Result<()> {
                Ok(())
            }
        }
    };
}
structure!(SerializeStruct);
structure!(SerializeStructVariant);

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn counting_stops_before_visiting_an_oversized_population() {
        struct Population<'a>(&'a Cell<usize>);
        impl Serialize for Population<'_> {
            fn serialize<S: ser::Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                use ser::SerializeSeq;
                let mut sequence = serializer.serialize_seq(Some(100_000))?;
                for _ in 0..100_000 {
                    self.0.set(self.0.get() + 1);
                    sequence.serialize_element("payload")?;
                }
                sequence.end()
            }
        }
        let visited = Cell::new(0);
        assert!(measure(&Population(&visited), 1024).is_err());
        assert!(visited.get() < 20);
    }

    #[test]
    fn small_structured_values_cover_the_decoded_representation() {
        let value = serde_json::json!({"nodes": [null, true, "abc", {"ticks": 50}]});
        assert!(measure(&value, 10_000).unwrap() >= super::super::decoded(&value));
        assert!(measure(&value, 32).is_err());
    }
}
