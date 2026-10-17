//! Byte-oriented bindings only. Network access and task state belong to the host.
use q2q_core::seed_builder::SeedBuilder as CoreBuilder;
use wasm_bindgen::prelude::*;

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
