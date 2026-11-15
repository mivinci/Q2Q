//! Byte-oriented bindings only. Network access and task state belong to the host.
use q2q_primitives::seed_builder::SeedBuilder as CoreBuilder;
use wasm_bindgen::prelude::*;

/// Inspection stays local to the caller; no source URL is ever fetched here.
#[wasm_bindgen]
pub fn decode_seed(bytes: &[u8]) -> Result<String, JsError> {
  let seed = q2q_primitives::seed::Seed::parse(bytes)?;
  seed.validate()?;
  let hex = |value: &[u8]| {
    value
      .iter()
      .map(|byte| format!("{byte:02x}"))
      .collect::<String>()
  };
  Ok(
    serde_json::json!({
      "name": seed.info.name,
      "size": seed.info.length.to_string(),
      "blockSize": seed.info.block_size,
      "blockCount": seed.info.leaf_count().to_string(),
      "infoHash": hex(&seed.info_hash()),
      "root": hex(&seed.info.root),
      "inlineProof": seed.leaf_hashes.is_some(),
      "tracks": seed.track_list,
      "sources": seed.cdn_list,
      "proofSources": seed.proof_list
    })
    .to_string(),
  )
}

/// Source hints live outside info; changing them must preserve content identity.
#[wasm_bindgen]
pub fn seed_with_network(
  bytes: &[u8],
  source: String,
  track: Option<String>,
) -> Result<Vec<u8>, JsError> {
  let encoded = seed_with_source(bytes, source)?;
  let mut seed = q2q_primitives::seed::Seed::parse(&encoded)?;
  seed.track_list = track
    .filter(|value| !value.is_empty())
    .into_iter()
    .collect();
  Ok(seed.encode())
}

/// Source hints live outside info; changing them must preserve content identity.
#[wasm_bindgen]
pub fn seed_with_source(bytes: &[u8], source: String) -> Result<Vec<u8>, JsError> {
  let mut seed = q2q_primitives::seed::Seed::parse(bytes)?;
  seed.validate()?;
  if seed.leaf_hashes.is_none()
    || source.len() > 4096
    || !(source.starts_with("https://") || source.starts_with("http://"))
  {
    return Err(JsError::new("inline seed and HTTP source required"));
  }
  seed.cdn_list = vec![source];
  seed.proof_list.clear();
  Ok(seed.encode())
}

/// Rebuild publication proof bytes from the validated, digest-pinned inline seed.
#[wasm_bindgen]
pub fn publication_proof(bytes: &[u8]) -> Result<Vec<u8>, JsError> {
  let seed = q2q_primitives::seed::Seed::parse(bytes)?;
  seed.validate()?;
  let leaves = seed
    .leaf_hashes
    .ok_or_else(|| JsError::new("inline proof required"))?;
  Ok(leaves.into_iter().flatten().collect())
}

#[wasm_bindgen]
pub struct SeedBuilder {
  inner: CoreBuilder,
}

#[wasm_bindgen]
impl SeedBuilder {
  #[wasm_bindgen(constructor)]
  pub fn new(length: f64) -> Result<Self, JsError> {
    // JS file sizes are Numbers. Reject lossy inputs before crossing into u64.
    if !length.is_finite()
      || length < 0.0
      || length.fract() != 0.0
      || length > 9_007_199_254_740_991.0
    {
      return Err(JsError::new("length must be a non-negative safe integer"));
    }
    Ok(Self {
      inner: CoreBuilder::new(length as u64)?,
    })
  }

  pub fn update(&mut self, bytes: &[u8]) -> Result<(), JsError> {
    Ok(self.inner.update(bytes)?)
  }

  /// Consumes the builder; the host chooses durable source URLs separately.
  pub fn finish(self, name: String) -> Result<PreparedSeed, JsError> {
    self.complete(name, false)
  }

  /// Standalone proof metadata for seeds downloaded before proof hosting is configured.
  pub fn finish_inline(self, name: String) -> Result<PreparedSeed, JsError> {
    self.complete(name, true)
  }
}

impl SeedBuilder {
  fn complete(self, name: String, inline: bool) -> Result<PreparedSeed, JsError> {
    let mut seed = self.inner.finish()?;
    seed.info.name = Some(name);
    let leaves = seed
      .leaf_hashes
      .take()
      .expect("builder returns leaf hashes");
    let mut proof = Vec::with_capacity(leaves.len() * 32);
    for leaf in &leaves {
      proof.extend_from_slice(leaf);
    }
    if inline {
      seed.leaf_hashes = Some(leaves);
    }
    Ok(PreparedSeed {
      seed: seed.encode(),
      proof,
      info_hash: seed.info_hash().to_vec(),
    })
  }
}

/// The proof is the protocol's complete raw leaf table, 32 bytes per block.
/// A Seed without inline leaves needs a durable proof_list URL before publication.
#[wasm_bindgen]
pub struct PreparedSeed {
  seed: Vec<u8>,
  proof: Vec<u8>,
  info_hash: Vec<u8>,
}

#[wasm_bindgen]
impl PreparedSeed {
  pub fn seed_bytes(&self) -> Vec<u8> {
    self.seed.clone()
  }
  pub fn proof_bytes(&self) -> Vec<u8> {
    self.proof.clone()
  }
  pub fn info_hash(&self) -> Vec<u8> {
    self.info_hash.clone()
  }
}

/// Seed-pinned block verification for browser download and recovery.
/// The leaf table is validated against the seed root once, never trusted alone.
#[wasm_bindgen]
pub struct BlockVerifier {
  info: q2q_primitives::seed::Info,
  leaves: Vec<q2q_primitives::merkle::Hash>,
  tree: q2q_primitives::merkle::ProofTree,
}

#[wasm_bindgen]
impl BlockVerifier {
  #[wasm_bindgen(constructor)]
  pub fn new(seed_bytes: &[u8], proof_bytes: &[u8]) -> Result<BlockVerifier, JsError> {
    if seed_bytes.len() > 17 * 1024 * 1024 || proof_bytes.len() > 16 * 1024 * 1024 {
      return Err(JsError::new("verification metadata exceeds budget"));
    }
    let seed = q2q_primitives::seed::Seed::parse(seed_bytes)?;
    if seed.info.length > 9_007_199_254_740_991 {
      return Err(JsError::new("file length exceeds browser integer range"));
    }
    seed.validate()?;
    // Bound both table size and the temporary tree used to verify its root.
    if seed.info.leaf_count() > 524_288 {
      return Err(JsError::new("block count exceeds verifier budget"));
    }
    let leaves = if proof_bytes.is_empty() {
      seed.leaf_hashes.unwrap_or_default()
    } else {
      if proof_bytes.len() != seed.info.leaf_count() * 32 {
        return Err(JsError::new("proof length mismatch"));
      }
      proof_bytes.as_chunks::<32>().0.to_vec()
    };
    if leaves.len() != seed.info.leaf_count()
      || q2q_primitives::merkle::root(&leaves) != seed.info.root
    {
      return Err(JsError::new("proof does not match seed"));
    }
    let tree = q2q_primitives::merkle::ProofTree::new(&leaves);
    Ok(Self {
      info: seed.info,
      leaves,
      tree,
    })
  }

  pub fn proof_frame(&self, index: u32) -> Result<String, JsError> {
    let proof = self
      .tree
      .proof(index as usize)
      .ok_or_else(|| JsError::new("invalid block"))?;
    q2q_primitives::peer_wire::Control::Proof {
      block_index: u64::from(index),
      count: 1,
      proof,
    }
    .encode()
    .map_err(|_| JsError::new("invalid proof"))
  }
  pub fn verify(&self, index: f64, bytes: &[u8]) -> bool {
    if !index.is_finite() || index < 0.0 || index.fract() != 0.0 {
      return false;
    }
    let index = index as usize;
    let Some(expected) = self.leaves.get(index) else {
      return false;
    };
    let offset = index as u64 * u64::from(self.info.block_size);
    let length = (self.info.length - offset).min(u64::from(self.info.block_size));
    bytes.len() as u64 == length && q2q_primitives::merkle::leaf_hash(bytes) == *expected
  }
}

/// Upload proof generation shares the native Merkle tree and exact wire encoding.
#[wasm_bindgen]
pub struct UploadProofs {
  verifier: BlockVerifier,
}
#[wasm_bindgen]
impl UploadProofs {
  #[wasm_bindgen(constructor)]
  pub fn new(seed: &[u8]) -> Result<UploadProofs, JsError> {
    let verifier = BlockVerifier::new(seed, &[])?;
    if verifier.leaves.len() > 65536 {
      return Err(JsError::new("upload block budget exceeded"));
    }
    Ok(Self { verifier })
  }
  pub fn verify(&self, index: u32, bytes: &[u8]) -> bool {
    self.verifier.verify(f64::from(index), bytes)
  }
  pub fn proof(&self, index: u32) -> Result<String, JsError> {
    let proof = self
      .verifier
      .tree
      .proof(index as usize)
      .ok_or_else(|| JsError::new("invalid upload block"))?;
    q2q_primitives::peer_wire::Control::Proof {
      block_index: u64::from(index),
      count: 1,
      proof,
    }
    .encode()
    .map_err(|_| JsError::new("invalid upload proof"))
  }
}

/// Magnet link parsing stays local; `tr` hints are validated but never dialed here.
/// Errors are stable machine codes so hosts can map them without string matching.
#[wasm_bindgen]
pub fn parse_magnet(link: &str) -> Result<String, JsError> {
  use q2q_primitives::seed::parse_magnet;
  let magnet = parse_magnet(link).map_err(|error| JsError::new(magnet_error_code(error)))?;
  let hex = |value: &[u8]| {
    value
      .iter()
      .map(|byte| format!("{byte:02x}"))
      .collect::<String>()
  };
  Ok(
    serde_json::json!({
      "infoHash": hex(&magnet.info_hash),
      "displayName": magnet.display_name,
      "trackers": magnet.trackers,
    })
    .to_string(),
  )
}

fn magnet_error_code(error: q2q_primitives::seed::MagnetError) -> &'static str {
  use q2q_primitives::seed::MagnetError;
  match error {
    MagnetError::NotAMagnet => "not_magnet",
    MagnetError::MissingXt => "missing_xt",
    MagnetError::WrongNamespace => "wrong_namespace",
    MagnetError::BadHash => "bad_hash",
    MagnetError::TooLong => "too_long",
    MagnetError::ConflictingXt => "conflicting_xt",
    MagnetError::BadEncoding => "bad_encoding",
  }
}

/// Verified peer metadata becomes a minimal seed: identity bound to `xt`, no
/// source hints. The hash is checked against the raw canonical info bytes.
#[wasm_bindgen]
pub fn magnet_seed(info_b64: &str, info_hash: &[u8]) -> Result<Vec<u8>, JsError> {
  use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
  use q2q_primitives::seed::MAX_INFO_BYTES;
  if info_b64.len() > MAX_INFO_BYTES.div_ceil(3) * 4 {
    return Err(JsError::new("info_too_large"));
  }
  let expected: [u8; 32] = info_hash.try_into().map_err(|_| JsError::new("bad_hash"))?;
  let bytes = B64
    .decode(info_b64)
    .map_err(|_| JsError::new("malformed_metadata"))?;
  let info = q2q_primitives::seed::Info::parse_verified(&bytes, &expected)
    .map_err(|error| JsError::new(seed_error_code(error)))?;
  Ok(q2q_primitives::seed::Seed::new(info).encode())
}

fn seed_error_code(error: q2q_primitives::seed::SeedError) -> &'static str {
  use q2q_primitives::seed::SeedError;
  match error {
    SeedError::InfoTooLarge => "info_too_large",
    SeedError::InfoHashMismatch => "info_hash_mismatch",
    _ => "malformed_metadata",
  }
}

/// The publisher's canonical info bytes, base64-encoded for `request_info`
/// responses. Oversized info is an error, not a truncation.
#[wasm_bindgen]
pub fn seed_info_b64(seed_bytes: &[u8]) -> Result<String, JsError> {
  use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
  let seed = q2q_primitives::seed::Seed::parse(seed_bytes)?;
  seed.validate()?;
  let encoded = seed.info.encode();
  if encoded.len() > q2q_primitives::seed::MAX_INFO_BYTES {
    return Err(JsError::new("info_too_large"));
  }
  Ok(B64.encode(encoded))
}

/// Share-side link generation from a local seed; pure and offline.
#[wasm_bindgen]
pub fn seed_magnet(seed_bytes: &[u8]) -> Result<String, JsError> {
  let seed = q2q_primitives::seed::Seed::parse(seed_bytes)?;
  seed.validate()?;
  Ok(q2q_primitives::seed::to_magnet(&seed))
}

mod control;

mod peer;

#[cfg(test)]
mod magnet_tests {
  use super::*;
  use q2q_primitives::seed::{Info, MagnetError, Seed, SeedError};

  fn fixture() -> (Seed, [u8; 32]) {
    let mut info = Info::new(1024, [7u8; 32]);
    info.name = Some("fixture.bin".to_owned());
    let hash = q2q_primitives::bencode::digest(&info.to_value());
    (Seed::new(info), hash)
  }

  #[test]
  fn magnet_error_codes_are_stable() {
    for (error, code) in [
      (MagnetError::NotAMagnet, "not_magnet"),
      (MagnetError::MissingXt, "missing_xt"),
      (MagnetError::WrongNamespace, "wrong_namespace"),
      (MagnetError::BadHash, "bad_hash"),
      (MagnetError::TooLong, "too_long"),
      (MagnetError::ConflictingXt, "conflicting_xt"),
      (MagnetError::BadEncoding, "bad_encoding"),
    ] {
      assert_eq!(magnet_error_code(error), code);
    }
    assert_eq!(seed_error_code(SeedError::InfoTooLarge), "info_too_large");
    assert_eq!(
      seed_error_code(SeedError::InfoHashMismatch),
      "info_hash_mismatch"
    );
    assert_eq!(
      seed_error_code(SeedError::RootWrongLength),
      "malformed_metadata"
    );
    assert_eq!(
      seed_error_code(SeedError::Malformed(
        q2q_primitives::bencode::DecodeError::UnexpectedEnd,
      )),
      "malformed_metadata"
    );
  }

  #[test]
  fn magnet_link_roundtrip_and_minimal_seed() {
    let (seed, hash) = fixture();
    let link = seed_magnet(&seed.encode()).unwrap();
    assert!(link.starts_with("magnet:?xt=urn:sha256:"));
    let parsed = q2q_primitives::seed::parse_magnet(&link).unwrap();
    assert_eq!(parsed.info_hash, hash);
    assert_eq!(parsed.display_name.as_deref(), Some("fixture.bin"));

    let info_b64 = seed_info_b64(&seed.encode()).unwrap();
    let minimal = magnet_seed(&info_b64, &hash).unwrap();
    let parsed = Seed::parse(&minimal).unwrap();
    assert_eq!(parsed.info_hash(), hash);
    assert_eq!(parsed.info.name.as_deref(), Some("fixture.bin"));
    assert!(
      parsed.cdn_list.is_empty()
        && parsed.proof_list.is_empty()
        && parsed.track_list.is_empty()
        && parsed.leaf_hashes.is_none()
    );
  }

  #[test]
  fn unknown_info_fields_preserve_identity_through_magnet() {
    // Unknown fields participate in the content identity; a magnet-resolved
    // seed must keep them instead of recomputing the hash from known fields.
    let (seed, hash) = fixture();
    let link = seed_magnet(&seed.encode()).unwrap();
    assert!(link.contains(&format!("urn:sha256:{}", hex(&hash))));
  }

  fn hex(value: &[u8]) -> String {
    value.iter().map(|byte| format!("{byte:02x}")).collect()
  }
}
