//! serde bindings for bencode.
//!
//! The point of this module is that `#[derive(Serialize)]` is safe to use: a
//! derived struct serialises its fields in declaration order, but canonical
//! bencode requires keys in ascending byte order. So the map and struct paths
//! buffer their entries and sort before emitting, which makes the output
//! independent of field order.

use crate::bencode::{BValue, Dict};
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::ser::{
  self, SerializeMap, SerializeSeq, SerializeStruct, SerializeTuple, SerializeTupleStruct,
};
use serde::{Deserialize, Serialize};
use std::fmt::Display;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
  Custom(String),
  /// bencode has no native representation for these
  Unsupported(&'static str),
  NotAStringKey,
  UnexpectedType(&'static str),
}

impl Display for Error {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      Error::Custom(s) => f.write_str(s),
      Error::Unsupported(t) => write!(f, "bencode cannot represent {t}"),
      Error::NotAStringKey => f.write_str("dict keys must be byte strings"),
      Error::UnexpectedType(t) => write!(f, "expected {t}"),
    }
  }
}

impl std::error::Error for Error {}

impl ser::Error for Error {
  fn custom<T: Display>(msg: T) -> Self {
    Error::Custom(msg.to_string())
  }
}

impl de::Error for Error {
  fn custom<T: Display>(msg: T) -> Self {
    Error::Custom(msg.to_string())
  }
}

/// Serialise any `Serialize` value into a `BValue`.
pub fn to_value<T: ?Sized + Serialize>(value: &T) -> Result<BValue, Error> {
  value.serialize(ValueSerializer)
}

/// Deserialise a `BValue` into any `Deserialize` type.
///
/// The deserializer owns its value: serde hands out `&'de` borrows, and dict
/// keys are produced on the fly, so a borrowed one could not satisfy that.
pub fn from_value<T: for<'de> Deserialize<'de>>(value: &BValue) -> Result<T, Error> {
  T::deserialize(ValueDeserializer {
    value: value.clone(),
  })
}

// ---------------------------------------------------------------- serializer

struct ValueSerializer;

impl ser::Serializer for ValueSerializer {
  type Ok = BValue;
  type Error = Error;
  type SerializeSeq = SeqBuilder;
  type SerializeTuple = SeqBuilder;
  type SerializeTupleStruct = SeqBuilder;
  type SerializeTupleVariant = ser::Impossible<BValue, Error>;
  type SerializeMap = MapBuilder;
  type SerializeStruct = MapBuilder;
  type SerializeStructVariant = ser::Impossible<BValue, Error>;

  fn serialize_bool(self, _: bool) -> Result<BValue, Error> {
    Err(Error::Unsupported("bool"))
  }
  fn serialize_i8(self, v: i8) -> Result<BValue, Error> {
    self.serialize_i64(v as i64)
  }
  fn serialize_i16(self, v: i16) -> Result<BValue, Error> {
    self.serialize_i64(v as i64)
  }
  fn serialize_i32(self, v: i32) -> Result<BValue, Error> {
    self.serialize_i64(v as i64)
  }
  fn serialize_i64(self, v: i64) -> Result<BValue, Error> {
    Ok(BValue::Int(v))
  }
  fn serialize_u8(self, v: u8) -> Result<BValue, Error> {
    self.serialize_i64(v as i64)
  }
  fn serialize_u16(self, v: u16) -> Result<BValue, Error> {
    self.serialize_i64(v as i64)
  }
  fn serialize_u32(self, v: u32) -> Result<BValue, Error> {
    self.serialize_i64(v as i64)
  }
  fn serialize_u64(self, v: u64) -> Result<BValue, Error> {
    if v > i64::MAX as u64 {
      return Err(Error::Unsupported("integer above i64::MAX"));
    }
    Ok(BValue::Int(v as i64))
  }
  fn serialize_f32(self, _: f32) -> Result<BValue, Error> {
    Err(Error::Unsupported("float"))
  }
  fn serialize_f64(self, _: f64) -> Result<BValue, Error> {
    Err(Error::Unsupported("float"))
  }
  fn serialize_char(self, v: char) -> Result<BValue, Error> {
    Ok(BValue::Bytes(v.to_string().into_bytes()))
  }
  fn serialize_str(self, v: &str) -> Result<BValue, Error> {
    Ok(BValue::Bytes(v.as_bytes().to_vec()))
  }
  fn serialize_bytes(self, v: &[u8]) -> Result<BValue, Error> {
    Ok(BValue::Bytes(v.to_vec()))
  }
  fn serialize_none(self) -> Result<BValue, Error> {
    // callers must use skip_serializing_if for optional fields
    Err(Error::Unsupported("None"))
  }
  fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<BValue, Error> {
    value.serialize(self)
  }
  fn serialize_unit(self) -> Result<BValue, Error> {
    Err(Error::Unsupported("unit"))
  }
  fn serialize_unit_struct(self, _: &'static str) -> Result<BValue, Error> {
    Err(Error::Unsupported("unit struct"))
  }
  fn serialize_unit_variant(
    self,
    _: &'static str,
    _: u32,
    _: &'static str,
  ) -> Result<BValue, Error> {
    Err(Error::Unsupported("enum variant"))
  }
  fn serialize_newtype_struct<T: ?Sized + Serialize>(
    self,
    _: &'static str,
    value: &T,
  ) -> Result<BValue, Error> {
    value.serialize(self)
  }
  fn serialize_newtype_variant<T: ?Sized + Serialize>(
    self,
    _: &'static str,
    _: u32,
    _: &'static str,
    _: &T,
  ) -> Result<BValue, Error> {
    Err(Error::Unsupported("enum variant"))
  }
  fn serialize_seq(self, len: Option<usize>) -> Result<SeqBuilder, Error> {
    Ok(SeqBuilder {
      items: Vec::with_capacity(len.unwrap_or(0)),
    })
  }
  fn serialize_tuple(self, len: usize) -> Result<SeqBuilder, Error> {
    self.serialize_seq(Some(len))
  }
  fn serialize_tuple_struct(self, _: &'static str, len: usize) -> Result<SeqBuilder, Error> {
    self.serialize_seq(Some(len))
  }
  fn serialize_tuple_variant(
    self,
    _: &'static str,
    _: u32,
    _: &'static str,
    _: usize,
  ) -> Result<Self::SerializeTupleVariant, Error> {
    Err(Error::Unsupported("enum variant"))
  }
  fn serialize_map(self, _: Option<usize>) -> Result<MapBuilder, Error> {
    Ok(MapBuilder {
      entries: Vec::new(),
    })
  }
  fn serialize_struct(self, _: &'static str, _: usize) -> Result<MapBuilder, Error> {
    Ok(MapBuilder {
      entries: Vec::new(),
    })
  }
  fn serialize_struct_variant(
    self,
    _: &'static str,
    _: u32,
    _: &'static str,
    _: usize,
  ) -> Result<Self::SerializeStructVariant, Error> {
    Err(Error::Unsupported("enum variant"))
  }
}

struct SeqBuilder {
  items: Vec<BValue>,
}

impl SerializeSeq for SeqBuilder {
  type Ok = BValue;
  type Error = Error;
  fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Error> {
    self.items.push(to_value(value)?);
    Ok(())
  }
  fn end(self) -> Result<BValue, Error> {
    Ok(BValue::List(self.items))
  }
}

impl SerializeTuple for SeqBuilder {
  type Ok = BValue;
  type Error = Error;
  fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Error> {
    SerializeSeq::serialize_element(self, value)
  }
  fn end(self) -> Result<BValue, Error> {
    SerializeSeq::end(self)
  }
}

impl SerializeTupleStruct for SeqBuilder {
  type Ok = BValue;
  type Error = Error;
  fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Error> {
    SerializeSeq::serialize_element(self, value)
  }
  fn end(self) -> Result<BValue, Error> {
    SerializeSeq::end(self)
  }
}

/// Buffers entries so they can be sorted by key before the dict is built.
struct MapBuilder {
  entries: Vec<(Vec<u8>, BValue)>,
}

impl MapBuilder {
  fn insert(&mut self, key: Vec<u8>, value: BValue) -> Result<(), Error> {
    self.entries.push((key, value));
    Ok(())
  }
}

impl SerializeMap for MapBuilder {
  type Ok = BValue;
  type Error = Error;
  fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<(), Error> {
    match to_value(key)? {
      BValue::Bytes(b) => self.insert(b, BValue::Int(0)),
      _ => Err(Error::NotAStringKey),
    }
  }
  fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Error> {
    // the placeholder written by serialize_key is replaced here
    let last = self.entries.last_mut().ok_or(Error::NotAStringKey)?;
    last.1 = to_value(value)?;
    Ok(())
  }
  fn end(self) -> Result<BValue, Error> {
    sorted_dict(self.entries)
  }
}

impl SerializeStruct for MapBuilder {
  type Ok = BValue;
  type Error = Error;
  fn serialize_field<T: ?Sized + Serialize>(
    &mut self,
    key: &'static str,
    value: &T,
  ) -> Result<(), Error> {
    self.insert(key.as_bytes().to_vec(), to_value(value)?)
  }
  fn end(self) -> Result<BValue, Error> {
    sorted_dict(self.entries)
  }
}

fn sorted_dict(mut entries: Vec<(Vec<u8>, BValue)>) -> Result<BValue, Error> {
  entries.sort_by(|a, b| a.0.cmp(&b.0));
  let mut d = Dict::new();
  for (k, v) in entries {
    d.insert(k, v);
  }
  Ok(BValue::Dict(d))
}

// -------------------------------------------------------------- deserializer

struct ValueDeserializer {
  value: BValue,
}

impl<'de> de::Deserializer<'de> for ValueDeserializer {
  type Error = Error;

  fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
    match self.value {
      BValue::Int(n) => visitor.visit_i64(n),
      BValue::Bytes(b) => visitor.visit_bytes(&b),
      BValue::List(items) => {
        let mut seq = SeqReader { items, index: 0 };
        visitor.visit_seq(&mut seq)
      }
      BValue::Dict(d) => {
        let mut keys = d.keys().cloned().collect::<Vec<_>>();
        keys.reverse();
        let mut map = MapReader {
          dict: d,
          keys,
          value: None,
        };
        visitor.visit_map(&mut map)
      }
    }
  }

  fn deserialize_bool<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Error> {
    Err(Error::Unsupported("bool"))
  }
  fn deserialize_i8<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Error> {
    self.deserialize_i64(v)
  }
  fn deserialize_i16<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Error> {
    self.deserialize_i64(v)
  }
  fn deserialize_i32<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Error> {
    self.deserialize_i64(v)
  }
  fn deserialize_i64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
    match self.value {
      BValue::Int(n) => visitor.visit_i64(n),
      _ => Err(Error::UnexpectedType("integer")),
    }
  }
  fn deserialize_u8<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Error> {
    self.deserialize_u64(v)
  }
  fn deserialize_u16<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Error> {
    self.deserialize_u64(v)
  }
  fn deserialize_u32<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Error> {
    self.deserialize_u64(v)
  }
  fn deserialize_u64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
    match self.value {
      BValue::Int(n) if n >= 0 => visitor.visit_u64(n as u64),
      BValue::Int(_) => Err(Error::UnexpectedType("non-negative integer")),
      _ => Err(Error::UnexpectedType("integer")),
    }
  }
  fn deserialize_f32<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Error> {
    Err(Error::Unsupported("float"))
  }
  fn deserialize_f64<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Error> {
    Err(Error::Unsupported("float"))
  }
  fn deserialize_char<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Error> {
    Err(Error::Unsupported("char"))
  }
  fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
    match self.value {
      BValue::Bytes(b) => visitor.visit_bytes(&b),
      _ => Err(Error::UnexpectedType("byte string")),
    }
  }
  fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
    self.deserialize_str(visitor)
  }
  fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
    match self.value {
      BValue::Bytes(b) => visitor.visit_bytes(&b),
      _ => Err(Error::UnexpectedType("byte string")),
    }
  }
  fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
    self.deserialize_bytes(visitor)
  }
  fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
    visitor.visit_some(self)
  }
  fn deserialize_unit<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Error> {
    Err(Error::Unsupported("unit"))
  }
  fn deserialize_unit_struct<V: Visitor<'de>>(
    self,
    _: &'static str,
    _: V,
  ) -> Result<V::Value, Error> {
    Err(Error::Unsupported("unit struct"))
  }
  fn deserialize_newtype_struct<V: Visitor<'de>>(
    self,
    _: &'static str,
    visitor: V,
  ) -> Result<V::Value, Error> {
    visitor.visit_newtype_struct(self)
  }
  fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
    match self.value {
      BValue::List(items) => {
        let mut seq = SeqReader { items, index: 0 };
        visitor.visit_seq(&mut seq)
      }
      _ => Err(Error::UnexpectedType("list")),
    }
  }
  fn deserialize_tuple<V: Visitor<'de>>(self, _: usize, visitor: V) -> Result<V::Value, Error> {
    self.deserialize_seq(visitor)
  }
  fn deserialize_tuple_struct<V: Visitor<'de>>(
    self,
    _: &'static str,
    _: usize,
    visitor: V,
  ) -> Result<V::Value, Error> {
    self.deserialize_seq(visitor)
  }
  fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
    match self.value {
      BValue::Dict(d) => {
        let mut keys = d.keys().cloned().collect::<Vec<_>>();
        keys.reverse();
        let mut map = MapReader {
          dict: d,
          keys,
          value: None,
        };
        visitor.visit_map(&mut map)
      }
      _ => Err(Error::UnexpectedType("dict")),
    }
  }
  fn deserialize_struct<V: Visitor<'de>>(
    self,
    _: &'static str,
    _: &'static [&'static str],
    visitor: V,
  ) -> Result<V::Value, Error> {
    self.deserialize_map(visitor)
  }
  fn deserialize_enum<V: Visitor<'de>>(
    self,
    _: &'static str,
    _: &'static [&'static str],
    _: V,
  ) -> Result<V::Value, Error> {
    Err(Error::Unsupported("enum"))
  }
  fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
    self.deserialize_str(visitor)
  }
  fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
    self.deserialize_any(visitor)
  }
}

struct SeqReader {
  items: Vec<BValue>,
  index: usize,
}

impl<'de> SeqAccess<'de> for SeqReader {
  type Error = Error;
  fn next_element_seed<T: DeserializeSeed<'de>>(
    &mut self,
    seed: T,
  ) -> Result<Option<T::Value>, Error> {
    match self.items.get(self.index) {
      Some(v) => {
        self.index += 1;
        seed
          .deserialize(ValueDeserializer { value: v.clone() })
          .map(Some)
      }
      None => Ok(None),
    }
  }
}

struct MapReader {
  dict: Dict,
  keys: Vec<Vec<u8>>,
  value: Option<BValue>,
}

impl<'de> MapAccess<'de> for MapReader {
  type Error = Error;
  fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>, Error> {
    match self.keys.pop() {
      Some(k) => {
        self.value = self.dict.get(&k[..]).cloned();
        seed
          .deserialize(ValueDeserializer {
            value: BValue::Bytes(k),
          })
          .map(Some)
      }
      None => Ok(None),
    }
  }
  fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Error> {
    let v = self
      .value
      .take()
      .ok_or(Error::UnexpectedType("dict value"))?;
    seed.deserialize(ValueDeserializer { value: v })
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use serde::{Deserialize as De, Serialize as Ser};

  #[derive(Ser, De, Debug, PartialEq)]
  struct Sample {
    // declared out of byte order on purpose
    #[serde(with = "serde_bytes")]
    root: Vec<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    length: u64,
    block_size: u32,
  }

  #[test]
  fn struct_fields_are_emitted_in_byte_order() {
    let s = Sample {
      root: vec![1, 2, 3],
      name: None,
      length: 42,
      block_size: 8,
    };
    let v = to_value(&s).unwrap();
    let text = String::from_utf8(v.encode()).unwrap();
    // block_size < length < root, and the absent name is not there at all
    assert_eq!(text, "d10:block_sizei8e6:lengthi42e4:root3:\x01\x02\x03e");
  }

  #[test]
  fn optional_field_is_omitted_when_none() {
    let without = Sample {
      root: vec![],
      name: None,
      length: 1,
      block_size: 1,
    };
    let text = String::from_utf8(to_value(&without).unwrap().encode()).unwrap();
    assert!(!text.contains("name"));

    let with = Sample {
      name: Some("x".into()),
      ..without
    };
    let text = String::from_utf8(to_value(&with).unwrap().encode()).unwrap();
    assert!(text.contains("4:name1:x"));
  }

  #[test]
  fn roundtrip_through_serde() {
    let s = Sample {
      root: vec![9; 32],
      name: Some("demo".into()),
      length: 1234,
      block_size: 256 * 1024,
    };
    let v = to_value(&s).unwrap();
    let back: Sample = from_value(&v).unwrap();
    assert_eq!(back, s);
  }

  #[test]
  fn byte_arrays_stay_binary() {
    // serde_bytes keeps [u8; 32] as one byte string, not 32 integers
    #[derive(Ser, De, Debug, PartialEq)]
    struct H {
      #[serde(with = "serde_bytes")]
      root: [u8; 32],
    }
    let h = H { root: [7u8; 32] };
    let v = to_value(&h).unwrap();
    match &v {
      BValue::Dict(d) => assert_eq!(d.get(&b"root"[..]), Some(&BValue::Bytes(vec![7u8; 32]))),
      other => panic!("expected dict, got {other:?}"),
    }
    assert_eq!(from_value::<H>(&v).unwrap(), h);
  }

  #[test]
  fn floats_are_rejected() {
    assert!(to_value(&1.5f64).is_err());
  }
}
