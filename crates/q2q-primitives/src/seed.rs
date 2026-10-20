//! Seed file, `info_hash` and magnet links.
//!
//! `info_hash` is SHA-256 over the canonical bencode of the info dict, so every
//! field that goes into it — including the presence or absence of optional ones
//! — is part of the content's identity.

use crate::bencode::{self, BValue, Dict};
use crate::merkle::{self, Hash};
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde::Serialize;
use sha2::{Digest, Sha256};

pub const INFO_VERSION: u32 = 2;
pub const MIN_BLOCK_SIZE: u32 = 256 * 1024;
pub const MAX_BLOCK_SIZE: u32 = 4 * 1024 * 1024;
pub const DEFAULT_BLOCK_SIZE: u32 = MIN_BLOCK_SIZE;
pub const MAX_INFO_BYTES: usize = 8 * 1024;
pub const MAX_MAGNET_BYTES: usize = 16 * 1024;

/// The info dict. `info_hash` is taken over its canonical encoding, so which
/// fields are present is part of the content's identity.
///
/// Construct with `Info::new`; parsed unknown fields are retained privately so
/// canonical encoding and serialization preserve the complete content identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
  /// Must be `INFO_VERSION`. Anything else is rejected outright.
  pub v: u32,
  /// Dropped from the encoding when absent, which changes `info_hash`.
  pub name: Option<String>,
  pub length: u64,
  /// Power of two within `[MIN_BLOCK_SIZE, MAX_BLOCK_SIZE]`.
  pub block_size: u32,
  pub mime: Option<String>,
  /// Merkle root, 32 raw bytes — not hex.
  pub root: Hash,
  // Unknown fields participate in identity and must survive parse/encode.
  extra: Dict,
}

impl Serialize for Info {
  fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
    self.to_value().serialize(serializer)
  }
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
      extra: Dict::new(),
    }
  }

  /// Number of leaves, i.e. `ceil(length / block_size)`.
  pub fn leaf_count(&self) -> usize {
    self.length.div_ceil(self.block_size as u64) as usize
  }

  /// Canonical bencode of the info dict: keys sorted by byte, optional fields
  /// omitted when absent.
  pub fn to_value(&self) -> BValue {
    let mut fields = self.extra.clone();
    fields.insert(b"v".to_vec(), BValue::Int(self.v.into()));
    fields.insert(b"length".to_vec(), BValue::Int(self.length as i64));
    fields.insert(b"block_size".to_vec(), BValue::Int(self.block_size.into()));
    fields.insert(b"root".to_vec(), BValue::bytes(self.root));
    if let Some(name) = &self.name {
      fields.insert(b"name".to_vec(), BValue::bytes(name.as_bytes()));
    }
    if let Some(mime) = &self.mime {
      fields.insert(b"mime".to_vec(), BValue::bytes(mime.as_bytes()));
    }
    BValue::Dict(fields)
  }

  pub fn encode(&self) -> Vec<u8> {
    self.to_value().encode()
  }

  /// Parse bounded canonical metadata and bind all fields to the requested identity.
  pub fn parse_verified(bytes: &[u8], expected: &Hash) -> Result<Self, SeedError> {
    if bytes.len() > MAX_INFO_BYTES {
      return Err(SeedError::InfoTooLarge);
    }
    let value = bencode::decode(bytes).map_err(SeedError::Malformed)?;
    let digest: Hash = Sha256::digest(bytes).into();
    if &digest != expected {
      return Err(SeedError::InfoHashMismatch);
    }
    let info = Self::from_value(&value)?;
    info.validate()?;
    if &info.info_hash() != expected {
      return Err(SeedError::InfoHashMismatch);
    }
    Ok(info)
  }

  pub fn info_hash(&self) -> Hash {
    bencode::digest(&self.to_value())
  }

  /// Checks the rules in spec 5.2 that concern info alone.
  pub fn validate(&self) -> Result<(), SeedError> {
    if self.length > i64::MAX as u64 {
      return Err(SeedError::FieldWrongType("length"));
    }
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
    // 上下界都要卡：负数和大数经 `as u32` 会回绕成别的值，
    // 报出来的错就不再是"这个字段不对"了
    let v_num = match d.get(&b"v"[..]) {
      Some(BValue::Int(n)) if *n >= 0 && *n <= u32::MAX as i64 => *n as u32,
      Some(_) => return Err(SeedError::FieldWrongType("v")),
      None => return Err(SeedError::MissingField("v")),
    };
    let length = match d.get(&b"length"[..]) {
      Some(BValue::Int(n)) if *n >= 0 => *n as u64,
      Some(_) => return Err(SeedError::FieldWrongType("length")),
      None => return Err(SeedError::MissingField("length")),
    };
    let block_size = match d.get(&b"block_size"[..]) {
      Some(BValue::Int(n)) if *n >= 0 && *n <= u32::MAX as i64 => *n as u32,
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
      v: v_num,
      name,
      length,
      block_size,
      mime,
      root,
      extra: d
        .iter()
        .filter(|(key, _)| {
          !matches!(
            key.as_slice(),
            b"v" | b"name" | b"length" | b"block_size" | b"mime" | b"root"
          )
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect(),
    })
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seed {
  pub track_list: Vec<String>,
  pub cdn_list: Vec<String>,
  /// HTTP(S) sources of the complete raw leaf table (32 bytes per block).
  pub proof_list: Vec<String>,
  pub leaf_hashes: Option<Vec<Hash>>,
  pub info: Info,
}

impl Seed {
  pub fn new(info: Info) -> Self {
    Seed {
      track_list: Vec::new(),
      cdn_list: Vec::new(),
      proof_list: Vec::new(),
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
    if !self.proof_list.is_empty() {
      d.insert(
        b"proof_list".to_vec(),
        BValue::List(
          self
            .proof_list
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
    let proof_list = string_list(d.get(&b"proof_list"[..]))?;
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
      proof_list,
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
  InfoTooLarge,
  InfoHashMismatch,
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
      SeedError::InfoTooLarge => f.write_str("info exceeds the 8 KiB metadata limit"),
      SeedError::InfoHashMismatch => f.write_str("info hash does not match the requested content"),
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
    // HTTPS is implicit in Q2Q tracker hints; HTTP must remain explicit.
    let hint = if t
      .get(..8)
      .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
    {
      &t[8..]
    } else {
      t
    };
    out.push_str(&utf8_percent_encode(hint, MAGNET_ENCODE).to_string());
  }
  out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MagnetError {
  NotAMagnet,
  MissingXt,
  WrongNamespace,
  BadHash,
  TooLong,
  ConflictingXt,
  BadEncoding,
}

impl std::fmt::Display for MagnetError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.write_str(match self {
      Self::NotAMagnet => "expected a magnet:? URL",
      Self::MissingXt => "magnet URL is missing xt",
      Self::WrongNamespace => "magnet xt must use urn:sha256 (BitTorrent links are unsupported)",
      Self::BadHash => "magnet info hash must contain 64 hexadecimal ASCII characters",
      Self::TooLong => "magnet URL exceeds the 16 KiB limit",
      Self::ConflictingXt => "magnet URL contains conflicting xt identities",
      Self::BadEncoding => "magnet URL has invalid percent encoding or UTF-8",
    })
  }
}
impl std::error::Error for MagnetError {}

pub fn parse_magnet(s: &str) -> Result<Magnet, MagnetError> {
  if s.len() > MAX_MAGNET_BYTES {
    return Err(MagnetError::TooLong);
  }
  if s.contains('#') {
    return Err(MagnetError::BadEncoding);
  }
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
      let decoded = percent_decode(v)?;
      let v = decoded.as_str();
      let hex_part = v
        .strip_prefix("urn:sha256:")
        .ok_or(MagnetError::WrongNamespace)?;
      let hash = decode_hex(hex_part).ok_or(MagnetError::BadHash)?;
      if info_hash.is_some_and(|previous| previous != hash) {
        return Err(MagnetError::ConflictingXt);
      }
      info_hash = Some(hash);
    } else if k == "dn" {
      if let Ok(name) = percent_decode(v) {
        display_name = Some(name);
      }
    } else if k == "tr" {
      if let Some(track) = percent_decode(v)
        .ok()
        .and_then(|hint| magnet_tracker(&hint))
      {
        if !trackers.contains(&track) {
          trackers.push(track);
        }
      }
    }
  }

  Ok(Magnet {
    info_hash: info_hash.ok_or(MagnetError::MissingXt)?,
    display_name,
    trackers,
  })
}

// Decode exactly once before adding the implicit scheme. Invalid optional hints
// remain ignorable; they do not change the content identity in xt.
fn magnet_tracker(hint: &str) -> Option<String> {
  if hint.is_empty() || hint.chars().any(char::is_whitespace) || hint.starts_with('/') {
    return None;
  }
  let track = if hint.contains("://") {
    hint.to_owned()
  } else {
    format!("https://{hint}")
  };
  let url = url::Url::parse(&track).ok()?;
  if !matches!(url.scheme(), "http" | "https")
    || url.host_str().is_none()
    || !url.username().is_empty()
    || url.password().is_some()
    || url.query().is_some()
    || url.fragment().is_some()
  {
    return None;
  }
  Some(track)
}

fn hex(bytes: &[u8]) -> String {
  bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn decode_hex(s: &str) -> Option<Hash> {
  if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
    return None;
  }
  let mut h = [0u8; 32];
  for (i, chunk) in h.iter_mut().enumerate() {
    *chunk = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
  }
  Some(h)
}

fn percent_decode(s: &str) -> Result<String, MagnetError> {
  let bytes = s.as_bytes();
  for (index, byte) in bytes.iter().enumerate() {
    if *byte == b'%'
      && (index + 2 >= bytes.len()
        || !bytes[index + 1..index + 3]
          .iter()
          .all(u8::is_ascii_hexdigit))
    {
      return Err(MagnetError::BadEncoding);
    }
  }
  percent_encoding::percent_decode_str(s)
    .decode_utf8()
    .map(|v| v.into_owned())
    .map_err(|_| MagnetError::BadEncoding)
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
      extra: Dict::new(),
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
  fn rejects_a_block_size_that_does_not_fit_in_u32() {
    let mut d = sample_info().to_value();
    if let BValue::Dict(m) = &mut d {
      m.insert(b"block_size".to_vec(), BValue::Int(1 << 33));
    }
    // 不卡上界的话 `as u32` 回绕成 0，报出来的会是 OutOfRange(0) 而不是字段不对
    assert!(matches!(
      Info::from_value(&d),
      Err(SeedError::FieldWrongType("block_size"))
    ));
  }

  #[test]
  fn rejects_a_negative_version() {
    let mut d = sample_info().to_value();
    if let BValue::Dict(m) = &mut d {
      m.insert(b"v".to_vec(), BValue::Int(-1));
    }
    assert!(matches!(
      Info::from_value(&d),
      Err(SeedError::FieldWrongType("v"))
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

#[cfg(test)]
mod proof_list_tests {
  use super::*;
  #[test]
  fn proof_urls_round_trip_without_changing_content_identity() {
    let mut seed = Seed::new(Info::new(0, merkle::EMPTY_ROOT));
    let hash = seed.info_hash();
    seed.proof_list = vec!["https://cdn.example.com/data.hashes".into()];
    let encoded = seed.encode();
    assert_eq!(Seed::parse(&encoded).unwrap(), seed);
    assert_eq!(seed.info_hash(), hash);
    let BValue::Dict(mut value) = bencode::decode(&encoded).unwrap() else {
      panic!()
    };
    value.insert(b"proof_list".to_vec(), BValue::Int(42));
    assert!(Seed::parse(&BValue::Dict(value).encode()).is_err());
  }
}

#[cfg(test)]
mod metadata_tests {
  use super::*;

  #[test]
  fn unknown_info_fields_survive_seed_metadata_and_serde_roundtrips() {
    assert_eq!(
      serde_json::to_value(Info::new(0, merkle::EMPTY_ROOT)).unwrap()["v"],
      INFO_VERSION
    );
    let mut value = Info::new(0, merkle::EMPTY_ROOT).to_value();
    let BValue::Dict(fields) = &mut value else {
      panic!()
    };
    fields.insert(
      b"future".to_vec(),
      BValue::List(vec![BValue::Bytes(vec![0xff, 0]), BValue::Int(3)]),
    );
    let hash = bencode::digest(&value);
    let info = Info::parse_verified(&value.encode(), &hash).unwrap();
    assert_eq!(info.info_hash(), hash);
    assert_eq!(crate::bencode_serde::to_value(&info).unwrap(), value);
    let seed = Seed::new(info);
    assert_eq!(Seed::parse(&seed.encode()).unwrap().info_hash(), hash);
    assert!(matches!(
      Info::parse_verified(&value.encode(), &[0; 32]),
      Err(SeedError::InfoHashMismatch)
    ));
  }

  #[test]
  fn malicious_metadata_and_magnets_are_bounded_and_never_panic() {
    assert!(bencode::decode(b"i+2e").is_err());
    let deep = [vec![b'l'; 1000], vec![b'e'; 1000]].concat();
    assert!(Info::parse_verified(&deep, &[0; 32]).is_err());
    assert!(matches!(
      Info::parse_verified(&vec![0; MAX_INFO_BYTES + 1], &[0; 32]),
      Err(SeedError::InfoTooLarge)
    ));
    let prefix = "magnet:?xt=urn:sha256:";
    for hash in ["aé".repeat(21) + "a", "z".repeat(64), "0".repeat(63)] {
      assert!(parse_magnet(&format!("{prefix}{hash}")).is_err());
    }
    let hash = "a".repeat(64);
    assert!(parse_magnet(&format!("{prefix}{hash}&xt=urn%3Asha256%3A{hash}")).is_ok());
    assert_eq!(
      parse_magnet(&format!("{prefix}{hash}&xt=urn:sha256:{}", "b".repeat(64))),
      Err(MagnetError::ConflictingXt)
    );
    assert_eq!(
      parse_magnet(&format!("{prefix}%zz")),
      Err(MagnetError::BadEncoding)
    );
    let hints = parse_magnet(&format!("{prefix}{hash}&dn=%zz&tr=%ff")).unwrap();
    assert!(hints.display_name.is_none() && hints.trackers.is_empty());
    assert_eq!(
      parse_magnet(&"m".repeat(MAX_MAGNET_BYTES + 1)),
      Err(MagnetError::TooLong)
    );
  }
}
