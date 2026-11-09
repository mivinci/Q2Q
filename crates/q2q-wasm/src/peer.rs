//! Channel identity and peer wire bindings; no host IO or WebRTC dependency.
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use ed25519_dalek::SigningKey;
use q2q_primitives::{
  identity::{sign_message, verify_signature},
  peer_auth::{fingerprint, IdentityFrame, Role, Transcript},
  peer_wire::{Control, Piece},
};
use wasm_bindgen::prelude::*;
fn error() -> JsError {
  JsError::new("invalid peer protocol input")
}

#[wasm_bindgen]
pub struct PeerAuthentication {
  key: SigningKey,
  remote: [u8; 32],
  initiator: bool,
  hash: [u8; 32],
  connection: String,
  nonce: [u8; 32],
  remote_nonce: Option<[u8; 32]>,
  local_fp: [u8; 32],
  remote_fp: [u8; 32],
  state: u8,
}
impl PeerAuthentication {
  #[allow(clippy::too_many_arguments)]
  pub(crate) fn new(
    key: SigningKey,
    remote: [u8; 32],
    initiator: bool,
    hash: [u8; 32],
    connection: String,
    nonce: [u8; 32],
    local_sdp: &str,
    remote_sdp: &str,
  ) -> Result<Self, JsError> {
    if local_sdp.len() > 65536 || remote_sdp.len() > 65536 {
      return Err(error());
    }
    Ok(Self {
      key,
      remote,
      initiator,
      hash,
      connection,
      nonce,
      remote_nonce: None,
      local_fp: fingerprint(local_sdp).map_err(|_| error())?,
      remote_fp: fingerprint(remote_sdp).map_err(|_| error())?,
      state: 0,
    })
  }
  fn transcript(&self) -> Transcript<'_> {
    let local = self.key.verifying_key().to_bytes();
    let remote_nonce = self.remote_nonce.expect("challenge accepted");
    let (
      initiator_key,
      responder_key,
      initiator_nonce,
      responder_nonce,
      initiator_fingerprint,
      responder_fingerprint,
    ) = if self.initiator {
      (
        local,
        self.remote,
        self.nonce,
        remote_nonce,
        self.local_fp,
        self.remote_fp,
      )
    } else {
      (
        self.remote,
        local,
        remote_nonce,
        self.nonce,
        self.remote_fp,
        self.local_fp,
      )
    };
    Transcript {
      initiator_key,
      responder_key,
      info_hash: self.hash,
      connection_id: &self.connection,
      initiator_nonce,
      responder_nonce,
      initiator_fingerprint,
      responder_fingerprint,
    }
  }
}
#[wasm_bindgen]
impl PeerAuthentication {
  pub fn hello(&self) -> Result<String, JsError> {
    if self.state != 0 {
      return Err(error());
    }
    IdentityFrame::Hello {
      v: 1,
      nonce: B64.encode(self.nonce),
    }
    .encode()
    .map_err(|_| error())
  }
  /// One hello then one proof. Any failure permanently invalidates this exchange.
  pub fn receive(&mut self, raw: &str) -> Result<Option<String>, JsError> {
    let state = self.state;
    self.state = 3;
    let frame = IdentityFrame::decode(raw.as_bytes()).map_err(|_| error())?;
    if state == 0 {
      self.remote_nonce = Some(frame.hello_nonce().map_err(|_| error())?);
      let role = if self.initiator {
        Role::Initiator
      } else {
        Role::Responder
      };
      let sig = sign_message(&self.key, &self.transcript().signing_bytes(role));
      let proof = IdentityFrame::Proof {
        v: 1,
        signature: B64.encode(sig),
      }
      .encode()
      .map_err(|_| error())?;
      self.state = 1;
      Ok(Some(proof))
    } else if state == 1 {
      let sig = frame.proof_signature().map_err(|_| error())?;
      let role = if self.initiator {
        Role::Responder
      } else {
        Role::Initiator
      };
      if !verify_signature(&self.remote, &self.transcript().signing_bytes(role), &sig) {
        return Err(error());
      }
      self.state = 2;
      Ok(None)
    } else {
      Err(error())
    }
  }
  pub fn authenticated(&self) -> bool {
    self.state == 2
  }
}
#[wasm_bindgen]
pub fn validate_peer_control(raw: &str) -> Result<(), JsError> {
  Control::decode(raw.as_bytes())
    .map(|_| ())
    .map_err(|_| error())
}
#[wasm_bindgen]
pub fn encode_peer_piece(block: u64, offset: u32, data: &[u8]) -> Result<Vec<u8>, JsError> {
  Piece {
    block_index: block,
    piece_offset: offset,
    data,
  }
  .encode()
  .map_err(|_| error())
}
#[wasm_bindgen]
pub fn peer_piece_header(raw: &[u8]) -> Result<String, JsError> {
  let piece = Piece::decode(raw).map_err(|_| error())?;
  Ok(serde_json::json!({"block_index":piece.block_index.to_string(),"piece_offset":piece.piece_offset}).to_string())
}

#[wasm_bindgen]
pub fn decode_peer_control(raw: &str) -> Result<String, JsError> {
  let message = Control::decode(raw.as_bytes()).map_err(|_| error())?;
  let mut value = serde_json::to_value(message)?;
  if let Some(index) = value.get("block_index").and_then(|index| index.as_u64()) {
    value["block_index"] = serde_json::Value::String(index.to_string());
  }
  Ok(serde_json::to_string(&value)?)
}
#[wasm_bindgen]
pub fn peer_bitfield(raw: &str, count: u32) -> Result<Vec<u8>, JsError> {
  let message = Control::decode(raw.as_bytes()).map_err(|_| error())?;
  let bits =
    q2q_primitives::peer_wire::decode_bitfield(message, count as usize).map_err(|_| error())?;
  Ok(bits.into_iter().map(u8::reverse_bits).collect())
}

#[wasm_bindgen]
pub struct PeerBlockAssembly(q2q_primitives::peer_wire::PieceAssembly);
#[wasm_bindgen]
impl PeerBlockAssembly {
  #[wasm_bindgen(constructor)]
  pub fn new(length: u32) -> Result<Self, JsError> {
    Ok(Self(
      q2q_primitives::peer_wire::PieceAssembly::new(length as usize).map_err(|_| error())?,
    ))
  }
  pub fn insert(&mut self, offset: u32, data: &[u8]) -> Result<bool, JsError> {
    self.0.insert(offset, data).map_err(|_| error())
  }
  pub fn complete(&self) -> bool {
    self.0.complete()
  }
  pub fn missing(&self) -> String {
    serde_json::to_string(&self.0.missing()).expect("bounded ranges")
  }
  pub fn bytes(&self) -> Result<Vec<u8>, JsError> {
    if !self.0.complete() {
      return Err(error());
    }
    Ok(self.0.bytes().to_vec())
  }
}

#[wasm_bindgen]
pub fn encode_peer_bitfield(bits: &[u8], count: u32) -> Result<String, JsError> {
  if count > 65536
    || bits.len() != (count as usize).div_ceil(8)
    || !count.is_multiple_of(8) && bits.last().is_some_and(|last| *last >> (count % 8) != 0)
  {
    return Err(error());
  }
  let wire: Vec<u8> = bits.iter().map(|byte| byte.reverse_bits()).collect();
  Control::Bitfield {
    bits: B64.encode(wire),
  }
  .encode()
  .map_err(|_| error())
}

/// Sparse verifier: only the seed root is trusted. Durable witnesses contain
/// sibling hashes, not URLs, tickets, target data or an unchecked trusted bit.
#[wasm_bindgen]
pub struct PeerProofVerifier {
  info: q2q_primitives::seed::Info,
}
#[wasm_bindgen]
impl PeerProofVerifier {
  #[wasm_bindgen(constructor)]
  pub fn new(seed: &[u8]) -> Result<Self, JsError> {
    if seed.len() > 17 * 1024 * 1024 {
      return Err(error());
    }
    let seed = q2q_primitives::seed::Seed::parse(seed)?;
    seed.validate()?;
    if seed.info.leaf_count() > 65536 || seed.info.length > 9_007_199_254_740_991 {
      return Err(error());
    }
    Ok(Self { info: seed.info })
  }
  pub fn witness(&self, index: u32, bytes: &[u8], raw: &str) -> Result<Vec<u8>, JsError> {
    let Control::Proof {
      block_index,
      count: 1,
      proof,
    } = Control::decode(raw.as_bytes()).map_err(|_| error())?
    else {
      return Err(error());
    };
    if block_index != u64::from(index) || !self.length(index, bytes) {
      return Err(error());
    }
    let proof = q2q_primitives::merkle::canonical_proof(
      bytes,
      index as usize,
      self.info.leaf_count(),
      &proof,
      &self.info.root,
    )
    .ok_or_else(error)?;
    let nodes = proof.nodes;
    let expected = self.path(index)?;
    if nodes.len() != expected.len() {
      return Err(error());
    }
    let mut output = b"Q2QW\x01".to_vec();
    output.push(nodes.len() as u8);
    for (node, (level, position)) in nodes.iter().zip(expected) {
      if node.level != level || node.index != position {
        return Err(error());
      }
      output.push(level as u8);
      output.extend_from_slice(&node.hash);
    }
    Ok(output)
  }
  /// Reconstruct an ordinary wire proof only after revalidating the actual bytes.
  pub fn proof_frame(&self, index: u32, bytes: &[u8], witness: &[u8]) -> Result<String, JsError> {
    if !self.verify(index, bytes, witness) {
      return Err(error());
    }
    let path = self.path(index)?;
    let nodes = witness[6..]
      .as_chunks::<33>()
      .0
      .iter()
      .zip(path)
      .map(|(chunk, (level, position))| q2q_primitives::merkle::Node {
        level,
        index: position,
        hash: chunk[1..].try_into().expect("exact witness node"),
      })
      .collect();
    Control::Proof {
      block_index: u64::from(index),
      count: 1,
      proof: q2q_primitives::merkle::Proof {
        leaf_hashes: vec![],
        nodes,
      },
    }
    .encode()
    .map_err(|_| error())
  }
  pub fn verify(&self, index: u32, bytes: &[u8], witness: &[u8]) -> bool {
    if !self.length(index, bytes)
      || witness.len() < 6
      || witness.len() > 640
      || &witness[..5] != b"Q2QW\x01"
    {
      return false;
    }
    let Ok(path) = self.path(index) else {
      return false;
    };
    if witness[5] as usize != path.len() || witness.len() != 6 + path.len() * 33 {
      return false;
    }
    let mut nodes = Vec::new();
    for (chunk, (level, position)) in witness[6..].as_chunks::<33>().0.iter().zip(path) {
      if chunk[0] != level as u8 {
        return false;
      }
      nodes.push(q2q_primitives::merkle::Node {
        level,
        index: position,
        hash: chunk[1..].try_into().expect("exact witness node"),
      });
    }
    q2q_primitives::merkle::verify(
      bytes,
      index as usize,
      self.info.leaf_count(),
      &q2q_primitives::merkle::Proof {
        leaf_hashes: vec![],
        nodes,
      },
      &self.info.root,
    )
  }
}
impl PeerProofVerifier {
  fn length(&self, index: u32, bytes: &[u8]) -> bool {
    let start = u64::from(index) * u64::from(self.info.block_size);
    (index as usize) < self.info.leaf_count()
      && bytes.len() as u64 == (self.info.length - start).min(u64::from(self.info.block_size))
  }
  fn path(&self, index: u32) -> Result<Vec<(u32, u64)>, JsError> {
    if index as usize >= self.info.leaf_count() {
      return Err(error());
    }
    let (mut count, mut position, mut level) = (self.info.leaf_count() as u64, u64::from(index), 0);
    let mut nodes = Vec::new();
    while count > 1 {
      let sibling = position ^ 1;
      if sibling < count {
        nodes.push((level, sibling));
      }
      count = count.div_ceil(2);
      position /= 2;
      level += 1;
    }
    Ok(nodes)
  }
}
