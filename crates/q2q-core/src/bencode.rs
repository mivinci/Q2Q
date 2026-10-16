//! Deterministic bencode: the basis of `info_hash` and ticket signatures.
//!
//! Bencode rather than JSON, for two reasons. It carries raw bytes, so `root`,
//! `pub` and `sig` need no hex or base64 round-trip; and determinism is trivial
//! to guarantee — no string escaping rules, no floating point to disagree about.
//!
//! Rules, frozen once published (changing any of them invalidates every
//! `info_hash` and signature already in the wild):
//!
//! 1. integers: `i<decimal>e`, no leading zeros, `-0` rejected
//! 2. byte strings: `<len>:<raw bytes>`, binary safe
//! 3. lists: `l…e`
//! 4. dicts: `d…e`, keys are byte strings in ascending byte order
//!
//! Rule 4 comes from `BTreeMap<Vec<u8>, _>`: ordering is a property of the type,
//! independent of insertion order. Decoding checks it and rejects input that
//! is valid bencode but not canonical.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub type Dict = BTreeMap<Vec<u8>, BValue>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BValue {
  Int(i64),
  Bytes(Vec<u8>),
  List(Vec<BValue>),
  Dict(Dict),
}

impl BValue {
  pub fn int(v: i64) -> Self {
    BValue::Int(v)
  }

  pub fn bytes(v: impl Into<Vec<u8>>) -> Self {
    BValue::Bytes(v.into())
  }

  pub fn encode(&self) -> Vec<u8> {
    let mut out = Vec::new();
    self.write_into(&mut out);
    out
  }

  fn write_into(&self, out: &mut Vec<u8>) {
    match self {
      BValue::Int(n) => {
        out.push(b'i');
        out.extend_from_slice(n.to_string().as_bytes());
        out.push(b'e');
      }
      BValue::Bytes(b) => {
        out.extend_from_slice(b.len().to_string().as_bytes());
        out.push(b':');
        out.extend_from_slice(b);
      }
      BValue::List(items) => {
        out.push(b'l');
        for item in items {
          item.write_into(out);
        }
        out.push(b'e');
      }
      BValue::Dict(map) => {
        out.push(b'd');
        for (k, v) in map {
          out.extend_from_slice(k.len().to_string().as_bytes());
          out.push(b':');
          out.extend_from_slice(k);
          v.write_into(out);
        }
        out.push(b'e');
      }
    }
  }
}

/// SHA-256 of the canonical encoding. `info_hash` and ticket signatures both go
/// through here, so any change to the encoding shows up in every digest.
pub fn digest(v: &BValue) -> [u8; 32] {
  let mut h = Sha256::new();
  h.update(v.encode());
  h.finalize().into()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
  UnexpectedEnd,
  TrailingData,
  InvalidByte(u8),
  InvalidInteger,
  InvalidLength,
  LeadingZero,
  NegativeZero,
  KeyNotString,
  KeysNotSorted,
}

impl std::fmt::Display for DecodeError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    let s = match self {
      DecodeError::UnexpectedEnd => "input ended mid-value",
      DecodeError::TrailingData => "trailing data after the value",
      DecodeError::InvalidByte(b) => return write!(f, "unexpected byte 0x{b:02x}"),
      DecodeError::InvalidInteger => "malformed integer",
      DecodeError::InvalidLength => "malformed byte string length",
      DecodeError::LeadingZero => "number has a leading zero",
      DecodeError::NegativeZero => "integer is negative zero",
      DecodeError::KeyNotString => "dict key is not a byte string",
      DecodeError::KeysNotSorted => "dict keys are not in ascending byte order",
    };
    f.write_str(s)
  }
}

impl std::error::Error for DecodeError {}

/// Decode bencode, rejecting anything that parses but is not canonical.
pub fn decode(input: &[u8]) -> Result<BValue, DecodeError> {
  let (v, n) = decode_at(input, 0)?;
  if n != input.len() {
    return Err(DecodeError::TrailingData);
  }
  Ok(v)
}

fn decode_at(input: &[u8], mut pos: usize) -> Result<(BValue, usize), DecodeError> {
  if pos >= input.len() {
    return Err(DecodeError::UnexpectedEnd);
  }
  match input[pos] {
    b'i' => {
      pos += 1;
      let start = pos;
      while pos < input.len() && input[pos] != b'e' {
        pos += 1;
      }
      if pos >= input.len() {
        return Err(DecodeError::UnexpectedEnd);
      }
      let text =
        std::str::from_utf8(&input[start..pos]).map_err(|_| DecodeError::InvalidInteger)?;
      let body = text.strip_prefix('-').unwrap_or(text);
      if body.is_empty() {
        return Err(DecodeError::InvalidInteger);
      }
      if body.len() > 1 && body.starts_with('0') {
        return Err(DecodeError::LeadingZero);
      }
      if text.starts_with("-0") {
        return Err(DecodeError::NegativeZero);
      }
      let n: i64 = text.parse().map_err(|_| DecodeError::InvalidInteger)?;
      Ok((BValue::Int(n), pos + 1))
    }

    b'0'..=b'9' => {
      let start = pos;
      while pos < input.len() && input[pos] != b':' {
        if !input[pos].is_ascii_digit() {
          return Err(DecodeError::InvalidLength);
        }
        pos += 1;
      }
      if pos >= input.len() {
        return Err(DecodeError::UnexpectedEnd);
      }
      let digits =
        std::str::from_utf8(&input[start..pos]).map_err(|_| DecodeError::InvalidLength)?;
      if digits.len() > 1 && digits.starts_with('0') {
        return Err(DecodeError::LeadingZero);
      }
      let len: usize = digits.parse().map_err(|_| DecodeError::InvalidLength)?;
      pos += 1; // skip the ':'
      if pos + len > input.len() {
        return Err(DecodeError::UnexpectedEnd);
      }
      Ok((BValue::Bytes(input[pos..pos + len].to_vec()), pos + len))
    }

    b'l' => {
      pos += 1;
      let mut items = Vec::new();
      loop {
        if pos >= input.len() {
          return Err(DecodeError::UnexpectedEnd);
        }
        if input[pos] == b'e' {
          pos += 1;
          break;
        }
        let (v, next) = decode_at(input, pos)?;
        items.push(v);
        pos = next;
      }
      Ok((BValue::List(items), pos))
    }

    b'd' => {
      pos += 1;
      let mut map = Dict::new();
      let mut last: Option<Vec<u8>> = None;
      loop {
        if pos >= input.len() {
          return Err(DecodeError::UnexpectedEnd);
        }
        if input[pos] == b'e' {
          pos += 1;
          break;
        }
        let (k, next) = decode_at(input, pos)?;
        let key = match k {
          BValue::Bytes(b) => b,
          _ => return Err(DecodeError::KeyNotString),
        };
        if let Some(prev) = &last {
          if key <= *prev {
            return Err(DecodeError::KeysNotSorted);
          }
        }
        last = Some(key.clone());
        let (v, next2) = decode_at(input, next)?;
        map.insert(key, v);
        pos = next2;
      }
      Ok((BValue::Dict(map), pos))
    }

    b => Err(DecodeError::InvalidByte(b)),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn dict(pairs: &[(&str, BValue)]) -> BValue {
    let mut m = Dict::new();
    for (k, v) in pairs {
      m.insert(k.as_bytes().to_vec(), v.clone());
    }
    BValue::Dict(m)
  }

  fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
  }

  #[test]
  fn int_encoding() {
    assert_eq!(BValue::Int(0).encode(), b"i0e");
    assert_eq!(BValue::Int(42).encode(), b"i42e");
    assert_eq!(BValue::Int(-42).encode(), b"i-42e");
    assert_eq!(
      BValue::Int(i64::MAX).encode(),
      format!("i{}e", i64::MAX).into_bytes()
    );
  }

  #[test]
  fn bytes_is_binary_safe() {
    // NUL and non-UTF-8 bytes must survive verbatim
    let raw = vec![0x00u8, 0xff, 0xfe];
    let enc = BValue::Bytes(raw.clone()).encode();
    assert_eq!(&enc[..2], b"3:");
    assert_eq!(&enc[2..], &raw[..]);
  }

  #[test]
  fn dict_keys_sorted_by_bytes() {
    // inserted with "b" first on purpose
    let v = dict(&[("b", BValue::Int(1)), ("a", BValue::Int(2))]);
    assert_eq!(String::from_utf8(v.encode()).unwrap(), "d1:ai2e1:bi1ee");
  }

  #[test]
  fn insertion_order_does_not_change_encoding() {
    let a = dict(&[("x", BValue::Int(1)), ("y", BValue::Int(2))]);
    let b = dict(&[("y", BValue::Int(2)), ("x", BValue::Int(1))]);
    assert_eq!(a.encode(), b.encode());
    assert_eq!(digest(&a), digest(&b));
  }

  #[test]
  fn nested_roundtrip() {
    let v = dict(&[(
      "info",
      dict(&[
        ("v", BValue::Int(2)),
        ("root", BValue::bytes(vec![0xab; 32])),
      ]),
    )]);
    let enc = v.encode();
    assert_eq!(decode(&enc).unwrap(), v);
  }

  #[test]
  fn known_vector() {
    // If the encoding ever shifts, this fails before any info_hash mismatch
    // shows up between two builds.
    let v = dict(&[("name", BValue::bytes("hello")), ("v", BValue::Int(2))]);
    assert_eq!(
      hex(&digest(&v)),
      "c2f58de7cbb326be3387cd033fbaf3737c7d8b4fdf6a3e583c1fde9c6b2af60f"
    );
  }

  #[test]
  fn rejects_leading_zero() {
    assert_eq!(decode(b"i042e"), Err(DecodeError::LeadingZero));
    assert_eq!(decode(b"04:abcd"), Err(DecodeError::LeadingZero));
  }

  #[test]
  fn rejects_negative_zero() {
    assert_eq!(decode(b"i-0e"), Err(DecodeError::NegativeZero));
  }

  #[test]
  fn rejects_unsorted_keys() {
    assert_eq!(decode(b"d1:bi1e1:ai2ee"), Err(DecodeError::KeysNotSorted));
  }

  #[test]
  fn rejects_trailing_data() {
    assert_eq!(decode(b"i1ei2e"), Err(DecodeError::TrailingData));
  }

  #[test]
  fn rejects_truncated() {
    assert_eq!(decode(b"5:abc"), Err(DecodeError::UnexpectedEnd));
    assert_eq!(decode(b"l"), Err(DecodeError::UnexpectedEnd));
  }
}
