//! A serde serializer into [`Canon`]. It has no floating-point numbers: a
//! float is refused as [`Error`] without any float arithmetic, so a module
//! that encodes only integers never contains a float instruction.
use super::tree::Canon;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;
use serde::ser::{self, Serialize};

/// Why a value could not be written.
#[derive(Debug)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl core::error::Error for Error {}

impl ser::Error for Error {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Error(msg.to_string())
    }
}

fn refused(what: &str) -> Error {
    Error(alloc::format!(
        "{what} cannot be written; only integers, text, booleans, lists and objects can"
    ))
}

/// The tree of `value`.
pub fn to_canon<T: Serialize + ?Sized>(value: &T) -> Result<Canon, Error> {
    value.serialize(Serializer)
}

pub struct Serializer;

pub struct Seq {
    items: Vec<Canon>,
    variant: Option<&'static str>,
}

pub struct Map {
    members: Vec<(String, Canon)>,
    key: Option<String>,
    variant: Option<&'static str>,
}

fn wrap(variant: Option<&'static str>, value: Canon) -> Canon {
    match variant {
        Some(name) => Canon::Obj(alloc::vec![(String::from(name), value)]),
        None => value,
    }
}

impl ser::Serializer for Serializer {
    type Ok = Canon;
    type Error = Error;
    type SerializeSeq = Seq;
    type SerializeTuple = Seq;
    type SerializeTupleStruct = Seq;
    type SerializeTupleVariant = Seq;
    type SerializeMap = Map;
    type SerializeStruct = Map;
    type SerializeStructVariant = Map;

    fn serialize_bool(self, v: bool) -> Result<Canon, Error> {
        Ok(Canon::Bool(v))
    }
    fn serialize_i8(self, v: i8) -> Result<Canon, Error> {
        Ok(Canon::Int(i64::from(v)))
    }
    fn serialize_i16(self, v: i16) -> Result<Canon, Error> {
        Ok(Canon::Int(i64::from(v)))
    }
    fn serialize_i32(self, v: i32) -> Result<Canon, Error> {
        Ok(Canon::Int(i64::from(v)))
    }
    fn serialize_i64(self, v: i64) -> Result<Canon, Error> {
        Ok(Canon::Int(v))
    }
    fn serialize_u8(self, v: u8) -> Result<Canon, Error> {
        Ok(Canon::Int(i64::from(v)))
    }
    fn serialize_u16(self, v: u16) -> Result<Canon, Error> {
        Ok(Canon::Int(i64::from(v)))
    }
    fn serialize_u32(self, v: u32) -> Result<Canon, Error> {
        Ok(Canon::Int(i64::from(v)))
    }
    fn serialize_u64(self, v: u64) -> Result<Canon, Error> {
        Ok(match i64::try_from(v) {
            Ok(i) => Canon::Int(i),
            Err(_) => Canon::UInt(v),
        })
    }
    fn serialize_f32(self, _: f32) -> Result<Canon, Error> {
        Err(refused("a floating-point number"))
    }
    fn serialize_f64(self, _: f64) -> Result<Canon, Error> {
        Err(refused("a floating-point number"))
    }
    fn serialize_char(self, v: char) -> Result<Canon, Error> {
        Ok(Canon::Str(v.to_string()))
    }
    fn serialize_str(self, v: &str) -> Result<Canon, Error> {
        Ok(Canon::Str(String::from(v)))
    }
    fn serialize_bytes(self, v: &[u8]) -> Result<Canon, Error> {
        Ok(Canon::Arr(
            v.iter().map(|b| Canon::Int(i64::from(*b))).collect(),
        ))
    }
    fn serialize_none(self) -> Result<Canon, Error> {
        Ok(Canon::Null)
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<Canon, Error> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<Canon, Error> {
        Ok(Canon::Null)
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<Canon, Error> {
        Ok(Canon::Null)
    }
    fn serialize_unit_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
    ) -> Result<Canon, Error> {
        Ok(Canon::Str(String::from(variant)))
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        value: &T,
    ) -> Result<Canon, Error> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Canon, Error> {
        Ok(wrap(Some(variant), value.serialize(Serializer)?))
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<Seq, Error> {
        Ok(Seq {
            items: Vec::with_capacity(len.unwrap_or(0)),
            variant: None,
        })
    }
    fn serialize_tuple(self, len: usize) -> Result<Seq, Error> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_struct(self, _: &'static str, len: usize) -> Result<Seq, Error> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Seq, Error> {
        Ok(Seq {
            items: Vec::with_capacity(len),
            variant: Some(variant),
        })
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Map, Error> {
        Ok(Map {
            members: Vec::new(),
            key: None,
            variant: None,
        })
    }
    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Map, Error> {
        self.serialize_map(None)
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        _: usize,
    ) -> Result<Map, Error> {
        Ok(Map {
            members: Vec::new(),
            key: None,
            variant: Some(variant),
        })
    }
}

impl Seq {
    fn push<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        self.items.push(value.serialize(Serializer)?);
        Ok(())
    }
    fn finish(self) -> Canon {
        wrap(self.variant, Canon::Arr(self.items))
    }
}

impl ser::SerializeSeq for Seq {
    type Ok = Canon;
    type Error = Error;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        self.push(value)
    }
    fn end(self) -> Result<Canon, Error> {
        Ok(self.finish())
    }
}

impl ser::SerializeTuple for Seq {
    type Ok = Canon;
    type Error = Error;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        self.push(value)
    }
    fn end(self) -> Result<Canon, Error> {
        Ok(self.finish())
    }
}

impl ser::SerializeTupleStruct for Seq {
    type Ok = Canon;
    type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        self.push(value)
    }
    fn end(self) -> Result<Canon, Error> {
        Ok(self.finish())
    }
}

impl ser::SerializeTupleVariant for Seq {
    type Ok = Canon;
    type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        self.push(value)
    }
    fn end(self) -> Result<Canon, Error> {
        Ok(self.finish())
    }
}

impl Map {
    fn member<T: Serialize + ?Sized>(&mut self, key: &str, value: &T) -> Result<(), Error> {
        self.members
            .push((String::from(key), value.serialize(Serializer)?));
        Ok(())
    }
    fn finish(self) -> Canon {
        wrap(self.variant, Canon::Obj(self.members))
    }
}

impl ser::SerializeMap for Map {
    type Ok = Canon;
    type Error = Error;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Error> {
        match key.serialize(Serializer)? {
            Canon::Str(s) => {
                self.key = Some(s);
                Ok(())
            }
            _ => Err(Error("an object key must be text".into())),
        }
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        let key = self
            .key
            .take()
            .ok_or_else(|| Error("an object value without its key".into()))?;
        self.members.push((key, value.serialize(Serializer)?));
        Ok(())
    }
    fn end(self) -> Result<Canon, Error> {
        Ok(self.finish())
    }
}

impl ser::SerializeStruct for Map {
    type Ok = Canon;
    type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        self.member(key, value)
    }
    fn end(self) -> Result<Canon, Error> {
        Ok(self.finish())
    }
}

impl ser::SerializeStructVariant for Map {
    type Ok = Canon;
    type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        self.member(key, value)
    }
    fn end(self) -> Result<Canon, Error> {
        Ok(self.finish())
    }
}
