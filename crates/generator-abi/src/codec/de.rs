//! A serde deserializer from [`Canon`]. Integers are offered as `i64` or
//! `u64` and nothing as a float, so deserializing never reaches
//! floating-point code. Enums are externally tagged (`"unit"` or
//! `{"variant": value}`), as serde writes them.
use super::ser::Error;
use super::tree::Canon;
use alloc::string::String;
use alloc::vec::{IntoIter, Vec};
use serde::de::{
    self, DeserializeOwned, DeserializeSeed, EnumAccess, IntoDeserializer, MapAccess, SeqAccess,
    VariantAccess, Visitor,
};

impl de::Error for Error {
    fn custom<T: core::fmt::Display>(msg: T) -> Self {
        Error(alloc::string::ToString::to_string(&msg))
    }

    // serde's own wording displays `Unexpected`, whose float case formats
    // an f64: these two say what was found without it, so no float
    // formatting code reaches a module.
    fn invalid_type(unexp: de::Unexpected<'_>, exp: &dyn de::Expected) -> Self {
        Error(alloc::format!(
            "invalid type: {}, expected {exp}",
            found(&unexp)
        ))
    }

    fn invalid_value(unexp: de::Unexpected<'_>, exp: &dyn de::Expected) -> Self {
        Error(alloc::format!(
            "invalid value: {}, expected {exp}",
            found(&unexp)
        ))
    }
}

/// What was found, in words, without formatting a float.
fn found(unexp: &de::Unexpected<'_>) -> String {
    use de::Unexpected as U;
    match unexp {
        U::Bool(b) => alloc::format!("boolean `{b}`"),
        U::Unsigned(v) => alloc::format!("integer `{v}`"),
        U::Signed(v) => alloc::format!("integer `{v}`"),
        U::Float(_) => "a number with a fraction".into(),
        U::Char(c) => alloc::format!("character `{c}`"),
        U::Str(s) => alloc::format!("string {s:?}"),
        U::Bytes(_) => "bytes".into(),
        U::Unit => "null".into(),
        U::Option => "an optional value".into(),
        U::NewtypeStruct => "a newtype struct".into(),
        U::Seq => "a list".into(),
        U::Map => "an object".into(),
        U::Enum => "an enum".into(),
        U::UnitVariant => "a unit variant".into(),
        U::NewtypeVariant => "a newtype variant".into(),
        U::TupleVariant => "a tuple variant".into(),
        U::StructVariant => "a struct variant".into(),
        U::Other(other) => String::from(*other),
    }
}

/// `T` read from `value`.
pub fn from_canon<T: DeserializeOwned>(value: Canon) -> Result<T, Error> {
    T::deserialize(value)
}

impl<'de> IntoDeserializer<'de, Error> for Canon {
    type Deserializer = Canon;
    fn into_deserializer(self) -> Canon {
        self
    }
}

struct Seq(IntoIter<Canon>);

impl<'de> SeqAccess<'de> for Seq {
    type Error = Error;
    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, Error> {
        match self.0.next() {
            Some(item) => seed.deserialize(item).map(Some),
            None => Ok(None),
        }
    }
    fn size_hint(&self) -> Option<usize> {
        Some(self.0.len())
    }
}

struct Map {
    members: IntoIter<(String, Canon)>,
    value: Option<Canon>,
}

impl<'de> MapAccess<'de> for Map {
    type Error = Error;
    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Error> {
        match self.members.next() {
            Some((key, value)) => {
                self.value = Some(value);
                seed.deserialize(Canon::Str(key)).map(Some)
            }
            None => Ok(None),
        }
    }
    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Error> {
        let value = self
            .value
            .take()
            .ok_or_else(|| Error("an object value without its key".into()))?;
        seed.deserialize(value)
    }
}

struct Enum {
    variant: String,
    value: Option<Canon>,
}

impl<'de> EnumAccess<'de> for Enum {
    type Error = Error;
    type Variant = Variant;
    fn variant_seed<V: DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, Variant), Error> {
        let name = seed.deserialize(Canon::Str(self.variant))?;
        Ok((name, Variant(self.value)))
    }
}

struct Variant(Option<Canon>);

impl<'de> VariantAccess<'de> for Variant {
    type Error = Error;
    fn unit_variant(self) -> Result<(), Error> {
        match self.0 {
            None => Ok(()),
            Some(_) => Err(Error("a variant without data was given data".into())),
        }
    }
    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, Error> {
        let value = self
            .0
            .ok_or_else(|| Error("a variant with data was given none".into()))?;
        seed.deserialize(value)
    }
    fn tuple_variant<V: Visitor<'de>>(self, _: usize, visitor: V) -> Result<V::Value, Error> {
        match self.0 {
            Some(Canon::Arr(items)) => visitor.visit_seq(Seq(items.into_iter())),
            _ => Err(Error("a tuple variant needs a list".into())),
        }
    }
    fn struct_variant<V: Visitor<'de>>(
        self,
        _: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        match self.0 {
            Some(Canon::Obj(members)) => visitor.visit_map(Map {
                members: members.into_iter(),
                value: None,
            }),
            _ => Err(Error("a struct variant needs an object".into())),
        }
    }
}

impl<'de> de::Deserializer<'de> for Canon {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self {
            Canon::Null => visitor.visit_unit(),
            Canon::Bool(b) => visitor.visit_bool(b),
            Canon::Int(i) => visitor.visit_i64(i),
            Canon::UInt(u) => visitor.visit_u64(u),
            Canon::Str(s) => visitor.visit_string(s),
            Canon::Arr(items) => visitor.visit_seq(Seq(items.into_iter())),
            Canon::Obj(members) => visitor.visit_map(Map {
                members: members.into_iter(),
                value: None,
            }),
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self {
            Canon::Null => visitor.visit_none(),
            other => visitor.visit_some(other),
        }
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _: &'static str,
        _: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        match self {
            Canon::Str(variant) => visitor.visit_enum(Enum {
                variant,
                value: None,
            }),
            Canon::Obj(members) => {
                let mut members: Vec<(String, Canon)> = members;
                match (members.pop(), members.is_empty()) {
                    (Some((variant, value)), true) => visitor.visit_enum(Enum {
                        variant,
                        value: Some(value),
                    }),
                    _ => Err(Error("a variant object has exactly one key".into())),
                }
            }
            _ => Err(Error("expected a variant".into())),
        }
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf unit unit_struct seq tuple tuple_struct map struct
        identifier ignored_any
    }
}
