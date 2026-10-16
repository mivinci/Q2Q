//! Seed file, `info_hash` and magnet links.
//!
//! `info_hash` is SHA-256 over the canonical bencode of the info dict, so every
//! field that goes into it — including the presence or absence of optional ones
//! — is part of the content's identity.

use crate::bencode::{self, BValue, Dict};
use crate::merkle::{self, Hash};
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde::Serialize;

pub const INFO_VERSION: u32 = 2;
pub const MIN_BLOCK_SIZE: u32 = 256 * 1024;
pub const MAX_BLOCK_SIZE: u32 = 4 * 1024 * 1024;
pub const DEFAULT_BLOCK_SIZE: u32 = MIN_BLOCK_SIZE;

/// The info dict. `info_hash` is taken over its canonical encoding, so which
/// fields are present is part of the content's identity.
///
/// Deriving `Serialize` is safe here: the bencode serialiser sorts keys by byte
/// before emitting, so the output does not depend on the field order below.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Info {
  /// Must be `INFO_VERSION`. Anything else is rejected outright.
  pub v: u32,
  /// Dropped from the encoding when absent, which changes `info_hash`.
  #[serde(skip_serializing_if = "Option::is_none")]
  pub name: Option<String>,
  pub length: u64,
  /// Power of two within `[MIN_BLOCK_SIZE, MAX_BLOCK_SIZE]`.
  pub block_size: u32,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub mime: Option<String>,
  /// Merkle root, 32 raw bytes — not hex.
  #[serde(with = "serde_bytes")]
  pub root: Hash,
}

impl Info {
  pub fn new(length: u64, root: Hash) -> Self {
    Info {
      v: INFO_VERSION,
      name: None,
      length,
      block_size: DEFAULT_BLOCK_SIZE,
      mime: None,
      root,
    }
  }

  /// Number of leaves, i.e. `ceil(length / block_size)`.
  pub fn leaf_count(&self) -> usize {
    self.length.div_ceil(self.block_size as u64) as usize
  }

  /// Canonical bencode of the info dict: keys sorted by byte, optional fields
  /// omitted when absent.
  pub fn to_value(&self) -> BValue {
    crate::bencode_serde::to_value(self).expect("info is always serialisable")
  }

  pub fn info_hash(&self) -> Hash {
    bencode::digest(&self.to_value())
  }

  /// Checks the rules in spec 5.2 that concern info alone.
  pub fn validate(&self) -> Result<(), SeedError> {
    if self.v != INFO_VERSION {
      return Err(SeedError::UnsupportedVersion(self.v));
    }
    if self.block_size < MIN_BLOCK_SIZE || self.block_size > MAX_BLOCK_SIZE {
      return Err(SeedError::BlockSizeOutOfRange(self.block_size));
    }
    if !self.block_size.is_power_of_two() {
      return Err(SeedError::BlockSizeNotPowerOfTwo(self.block_size));
    }
    Ok(())
  }

  fn from_value(v: &BValue) -> Result<Self, SeedError> {
    let d = match v {
      BValue::Dict(d) => d,
      _ => return Err(SeedError::InfoNotADict),
    };
    let v_num = match d.get(&b"v"[..]) {
      Some(BValue::Int(n)) => *n,
      Some(_) => return Err(SeedError::FieldWrongType("v")),
      None => return Err(SeedError::MissingField("v")),
    };
    let length = match d.get(&b"length"[..]) {
      Some(BValue::Int(n)) if *n >= 0 => *n as u64,
      Some(_) => return Err(SeedError::FieldWrongType("length")),
      None => return Err(SeedError::MissingField("length")),
    };
    let block_size = match d.get(&b"block_size"[..]) {
      Some(BValue::Int(n)) if *n >= 0 => *n as u32,
      Some(_) => return Err(SeedError::FieldWrongType("block_size")),
      None => return Err(SeedError::MissingField("block_size")),
    };
    let root: Hash = match d.get(&b"root"[..]) {
      Some(BValue::Bytes(b)) if b.len() == 32 => {
        let mut h = [0u8; 32];
        h.copy_from_slice(b);
        h
      }
      Some(_) => return Err(SeedError::RootWrongLength),
      None => return Err(SeedError::MissingField("root")),
    };

    let name = match d.get(&b"name"[..]) {
      Some(BValue::Bytes(b)) => {
        Some(String::from_utf8(b.clone()).map_err(|_| SeedError::FieldWrongType("name"))?)
      }
      None => None,
      Some(_) => return Err(SeedError::FieldWrongType("name")),
    };
    let mime = match d.get(&b"mime"[..]) {
      Some(BValue::Bytes(b)) => {
        Some(String::from_utf8(b.clone()).map_err(|_| SeedError::FieldWrongType("mime"))?)
      }
      None => None,
      Some(_) => return Err(SeedError::FieldWrongType("mime")),
    };

    Ok(Info {
      v: v_num as u32,
      name,
      length,
      block_size,
      mime,
      root,
    })
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seed {
  pub track_list: Vec<String>,
  pub cdn_list: Vec<String>,
  pub leaf_hashes: Option<Vec<Hash>>,
  pub info: Info,
}

impl Seed {
  pub fn new(info: Info) -> Self {
    Seed {
      track_list: Vec::new(),
      cdn_list: Vec::new(),
      leaf_hashes: None,
      info,
    }
  }

  pub fn info_hash(&self) -> Hash {
    self.info.info_hash()
  }

  pub fn encode(&self) -> Vec<u8> {
    let mut d = Dict::new();
    if !self.track_list.is_empty() {
      d.insert(
        b"track_list".to_vec(),
        BValue::List(
          self
            .track_list
            .iter()
            .map(|s| BValue::bytes(s.as_bytes()))
            .collect(),
        ),
      );
    }
    if !self.cdn_list.is_empty() {
      d.insert(
        b"cdn_list".to_vec(),
        BValue::List(
          self
            .cdn_list
            .iter()
            .map(|s| BValue::bytes(s.as_bytes()))
            .collect(),
        ),
      );
    }
    if let Some(hashes) = &self.leaf_hashes {
      d.insert(
        b"leaf_hashes".to_vec(),
        BValue::Bytes(hashes.iter().flat_map(|h| h.iter().copied()).collect()),
      );
    }
    d.insert(b"info".to_vec(), self.info.to_value());
    BValue::Dict(d).encode()
  }

  pub fn parse(bytes: &[u8]) -> Result<Self, SeedError> {
    let value = bencode::decode(bytes).map_err(SeedError::Malformed)?;
    let d = match &value {
      BValue::Dict(d) => d,
      _ => {
        return Err(SeedError::Malformed(bencode::DecodeError::InvalidByte(
          b'x',
        )))
      }
    };

    let track_list = string_list(d.get(&b"track_list"[..]))?;
    let cdn_list = string_list(d.get(&b"cdn_list"[..]))?;
    let leaf_hashes = match d.get(&b"leaf_hashes"[..]) {
      Some(BValue::Bytes(b)) => {
        if b.len() % 32 != 0 {
          return Err(SeedError::LeafHashesWrongLength(b.len()));
        }
        Some(
          b.chunks(32)
            .map(|c| {
              let mut h = [0u8; 32];
              h.copy_from_slice(c);
              h
            })
            .collect(),
        )
      }
      None => None,
      Some(_) => return Err(SeedError::FieldWrongType("leaf_hashes")),
    };

    let info = match d.get(&b"info"[..]) {
      Some(v) => Info::from_value(v)?,
      None => return Err(SeedError::MissingField("info")),
    };
    info.validate()?;

    let seed = Seed {
      track_list,
      cdn_list,
      leaf_hashes,
      info,
    };
    seed.validate()?;
    Ok(seed)
  }

  /// The remaining rules of spec 5.2: leaf_hashes must cover exactly `n` leaves
  /// and must rebuild the root in `info`.
  pub fn validate(&self) -> Result<(), SeedError> {
    self.info.validate()?;
    if let Some(hashes) = &self.leaf_hashes {
      let n = self.info.leaf_count();
      if hashes.len() != n {
        return Err(SeedError::LeafHashesWrongLength(hashes.len()));
      }
      if merkle::root(hashes) != self.info.root {
        return Err(SeedError::LeafHashesMismatch);
      }
    }
    Ok(())
  }
}

fn string_list(v: Option<&BValue>) -> Result<Vec<String>, SeedError> {
  match v {
    None => Ok(Vec::new()),
    Some(BValue::List(items)) => items
      .iter()
      .map(|i| match i {
        BValue::Bytes(b) => {
          String::from_utf8(b.clone()).map_err(|_| SeedError::FieldWrongType("url"))
        }
        _ => Err(SeedError::FieldWrongType("url")),
      })
      .collect(),
    Some(_) => Err(SeedError::FieldWrongType("list")),
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeedError {
  Malformed(bencode::DecodeError),
  InfoNotADict,
  MissingField(&'static str),
  FieldWrongType(&'static str),
  UnsupportedVersion(u32),
  BlockSizeOutOfRange(u32),
  BlockSizeNotPowerOfTwo(u32),
  RootWrongLength,
  LeafHashesWrongLength(usize),
  LeafHashesMismatch,
}

impl std::fmt::Display for SeedError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      SeedError::Malformed(e) => write!(f, "malformed seed: {e}"),
      SeedError::InfoNotADict => f.write_str("info is not a dict"),
      SeedError::MissingField(k) => write!(f, "missing field: {k}"),
      SeedError::FieldWrongType(k) => write!(f, "wrong type for field: {k}"),
      SeedError::UnsupportedVersion(v) => write!(f, "unsupported info version: {v}"),
      SeedError::BlockSizeOutOfRange(s) => write!(f, "block_size out of range: {s}"),
      SeedError::BlockSizeNotPowerOfTwo(s) => write!(f, "block_size is not a power of two: {s}"),
      SeedError::RootWrongLength => f.write_str("root must be 32 bytes"),
      SeedError::LeafHashesWrongLength(n) => write!(
        f,
        "leaf_hashes covers {n} leaves, expected a count matching length/block_size"
      ),
      SeedError::LeafHashesMismatch => f.write_str("leaf_hashes does not rebuild info.root"),
    }
  }
}

impl std::error::Error for SeedError {}

/// A magnet link: `xt` is the only normative part, `dn` and `tr` are hints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Magnet {
  pub info_hash: Hash,
  pub display_name: Option<String>,
  pub trackers: Vec<String>,
}

const MAGNET_ENCODE: &AsciiSet = &NON_ALPHANUMERIC
  .remove(b'-')
  .remove(b'.')
  .remove(b'_')
  .remove(b'~');

pub fn to_magnet(seed: &Seed) -> String {
  let mut out = String::from("magnet:?xt=urn:sha256:");
  out.push_str(&hex(&seed.info_hash()));
  if let Some(name) = &seed.info.name {
    out.push_str("&dn=");
    out.push_str(&utf8_percent_encode(name, MAGNET_ENCODE).to_string());
  }
  for t in &seed.track_list {
    out.push_str("&tr=");
    out.push_str(&utf8_percent_encode(t, MAGNET_ENCODE).to_string());
  }
  out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MagnetError {
  NotAMagnet,
  MissingXt,
  WrongNamespace,
  BadHash,
}

pub fn parse_magnet(s: &str) -> Result<Magnet, MagnetError> {
  let rest = s.strip_prefix("magnet:?").ok_or(MagnetError::NotAMagnet)?;
  let mut info_hash = None;
  let mut display_name = None;
  let mut trackers = Vec::new();

  for param in rest.split('&') {
    let (k, v) = match param.split_once('=') {
      Some((k, v)) => (k, v),
      None => continue,
    };
    if k == "xt" {
      let hex_part = v
        .strip_prefix("urn:sha256:")
        .ok_or(MagnetError::WrongNamespace)?;
      info_hash = Some(decode_hex(hex_part).ok_or(MagnetError::BadHash)?);
    } else if k == "dn" {
      display_name = Some(percent_decode(v));
    } else if k == "tr" {
      trackers.push(percent_decode(v));
    }
  }

  Ok(Magnet {
    info_hash: info_hash.ok_or(MagnetError::MissingXt)?,
    display_name,
    trackers,
  })
}

fn hex(bytes: &[u8]) -> String {
  bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn decode_hex(s: &str) -> Option<Hash> {
  if s.len() != 64 {
    return None;
  }
  let mut h = [0u8; 32];
  for (i, chunk) in h.iter_mut().enumerate() {
    *chunk = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
  }
  Some(h)
}

fn percent_decode(s: &str) -> String {
  let bytes = percent_encoding::percent_decode_str(s).collect::<Vec<u8>>();
  String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::merkle::root as tree_root;

  fn sample_info() -> Info {
    Info {
      v: INFO_VERSION,
      name: Some("demo.bin".into()),
      length: 700_000,
      block_size: DEFAULT_BLOCK_SIZE,
      mime: None,
      root: [0x11u8; 32],
    }
  }

  #[test]
  fn info_hash_is_stable() {
    let a = sample_info().info_hash();
    let b = sample_info().info_hash();
    assert_eq!(a, b);
    assert_eq!(a.len(), 32);
  }

  #[test]
  fn optional_fields_are_omitted() {
    let mut info = sample_info();
    info.name = None;
    info.mime = None;
    let encoded = info.to_value().encode();
    let text = String::from_utf8_lossy(&encoded);
    assert!(!text.contains("name"));
    assert!(!text.contains("mime"));
  }

  #[test]
  fn omitting_a_field_changes_the_hash() {
    let mut with = sample_info();
    let mut without = sample_info();
    without.name = None;
    assert_ne!(with.info_hash(), without.info_hash());
    let _ = &mut with;
  }

  #[test]
  fn leaf_count_rounds_up() {
    let info = sample_info(); // 700_000 / 262_144
    assert_eq!(info.leaf_count(), 3);
    let whole = Info::new(DEFAULT_BLOCK_SIZE as u64 * 4, [0u8; 32]);
    assert_eq!(whole.leaf_count(), 4);
  }

  #[test]
  fn seed_roundtrip() {
    let mut seed = Seed::new(sample_info());
    seed.track_list = vec!["https://t.example.com".into()];
    seed.cdn_list = vec!["https://cdn.example.com/x.bin".into()];
    let bytes = seed.encode();
    let parsed = Seed::parse(&bytes).unwrap();
    assert_eq!(parsed, seed);
    assert_eq!(parsed.info_hash(), seed.info_hash());
  }

  #[test]
  fn rejects_wrong_version() {
    let mut info = sample_info();
    info.v = 1;
    assert_eq!(info.validate(), Err(SeedError::UnsupportedVersion(1)));
  }

  #[test]
  fn rejects_block_size_not_power_of_two() {
    let mut info = sample_info();
    info.block_size = 300 * 1024;
    assert_eq!(
      info.validate(),
      Err(SeedError::BlockSizeNotPowerOfTwo(300 * 1024))
    );
  }

  #[test]
  fn rejects_block_size_out_of_range() {
    let mut info = sample_info();
    info.block_size = 8 * 1024 * 1024;
    assert!(matches!(
      info.validate(),
      Err(SeedError::BlockSizeOutOfRange(_))
    ));
  }

  #[test]
  fn rejects_short_root() {
    let mut d = sample_info().to_value();
    if let BValue::Dict(m) = &mut d {
      m.insert(b"root".to_vec(), BValue::Bytes(vec![0u8; 31]));
    }
    assert!(matches!(
      Info::from_value(&d),
      Err(SeedError::RootWrongLength)
    ));
  }

  #[test]
  fn rejects_leaf_hashes_of_wrong_length() {
    let mut seed = Seed::new(sample_info()); // 3 leaves
    seed.leaf_hashes = Some(vec![[0u8; 32]; 4]);
    assert!(matches!(
      seed.validate(),
      Err(SeedError::LeafHashesWrongLength(4))
    ));
  }

  #[test]
  fn rejects_leaf_hashes_not_matching_root() {
    let mut seed = Seed::new(sample_info());
    let hashes = vec![[1u8; 32], [2u8; 32], [3u8; 32]];
    seed.leaf_hashes = Some(hashes.clone());
    seed.info.root = tree_root(&hashes);
    assert!(seed.validate().is_ok());

    seed.info.root = [0xffu8; 32];
    assert_eq!(seed.validate(), Err(SeedError::LeafHashesMismatch));
  }

  #[test]
  fn magnet_roundtrip() {
    let mut seed = Seed::new(sample_info());
    seed.track_list = vec!["https://t.example.com/announce".into()];
    let link = to_magnet(&seed);
    assert!(link.starts_with("magnet:?xt=urn:sha256:"));
    let m = parse_magnet(&link).unwrap();
    assert_eq!(m.info_hash, seed.info_hash());
    assert_eq!(m.display_name.as_deref(), Some("demo.bin"));
    assert_eq!(m.trackers, seed.track_list);
  }

  #[test]
  fn magnet_percent_encodes_the_name() {
    let mut seed = Seed::new(sample_info());
    seed.info.name = Some("a b&c.bin".into());
    let link = to_magnet(&seed);
    assert!(link.contains("&dn=a%20b%26c.bin"));
    let m = parse_magnet(&link).unwrap();
    assert_eq!(m.display_name.as_deref(), Some("a b&c.bin"));
  }

  #[test]
  fn magnet_xt_is_the_only_required_part() {
    let link = format!("magnet:?xt=urn:sha256:{}", hex(&[0xab; 32]));
    let m = parse_magnet(&link).unwrap();
    assert_eq!(m.info_hash, [0xab; 32]);
    assert_eq!(m.display_name, None);
    assert!(m.trackers.is_empty());
  }

  #[test]
  fn rejects_foreign_namespace() {
    let link = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567";
    assert_eq!(parse_magnet(link), Err(MagnetError::WrongNamespace));
  }
}
