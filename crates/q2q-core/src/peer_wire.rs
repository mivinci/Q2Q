//! Versioned peer control messages and MTU-sized binary pieces.

use crate::block::PIECE_SIZE;
use crate::merkle::Proof;
use serde::{Deserialize, Serialize};

pub const MAX_CONTROL_BYTES: usize = 16 * 1024;
pub const MAX_BLOCKS: usize = 65_536;
pub const PIECE_HEADER: usize = 17;
const VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Control {
  Bitfield {
    bits: String,
  },
  Have {
    block_index: u64,
  },
  Request {
    block_index: u64,
    count: u32,
  },
  RequestProof {
    block_index: u64,
    count: u32,
  },
  Proof {
    block_index: u64,
    count: u32,
    proof: Proof,
  },
  UploadWait {
    block_index: u64,
    retry_after: u32,
  },
  PieceNack {
    block_index: u64,
    piece_offset: Vec<u32>,
    length: Vec<u32>,
  },
  BlockDone {
    block_index: u64,
  },
  Cancel {
    block_index: u64,
    count: u32,
  },
  Reject {
    block_index: u64,
    count: u32,
    retry_after: u32,
  },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
  v: u32,
  message: Control,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireError(pub &'static str);

impl std::fmt::Display for WireError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.write_str(self.0)
  }
}

impl std::error::Error for WireError {}

impl Control {
  pub fn encode(&self) -> Result<String, WireError> {
    #[derive(Serialize)]
    struct RefEnvelope<'a> {
      v: u32,
      message: &'a Control,
    }
    let raw = serde_json::to_string(&RefEnvelope {
      v: VERSION,
      message: self,
    })
    .map_err(|_| WireError("invalid control message"))?;
    if raw.len() > MAX_CONTROL_BYTES {
      return Err(WireError("control message too large"));
    }
    Ok(raw)
  }

  pub fn decode(raw: &[u8]) -> Result<Self, WireError> {
    if raw.len() > MAX_CONTROL_BYTES {
      return Err(WireError("control message too large"));
    }
    let envelope: Envelope =
      serde_json::from_slice(raw).map_err(|_| WireError("invalid control message"))?;
    if envelope.v != VERSION {
      return Err(WireError("unsupported peer protocol version"));
    }
    match &envelope.message {
      Self::UploadWait { retry_after, .. } if *retry_after == 0 || *retry_after > 30_000 => {
        return Err(WireError("invalid upload wait duration"));
      }
      Self::Proof { proof, count, .. }
        if *count != 1 || proof.nodes.len() > 64 || proof.leaf_hashes.len() > 1 =>
      {
        return Err(WireError("proof exceeds the single-block window"))
      }
      Self::PieceNack {
        piece_offset,
        length,
        ..
      } if piece_offset.is_empty()
        || piece_offset.len() > 64
        || piece_offset.len() != length.len() =>
      {
        return Err(WireError("invalid missing-piece ranges"))
      }
      _ => {}
    }
    Ok(envelope.message)
  }
}

/// One DataChannel message; the payload is at most 1024 bytes. Offsets are
/// byte offsets within the block, not piece indices.
pub struct Piece<'a> {
  pub block_index: u64,
  pub piece_offset: u32,
  pub data: &'a [u8],
}

impl Piece<'_> {
  pub fn encode(&self) -> Result<Vec<u8>, WireError> {
    if self.data.is_empty()
      || self.data.len() > PIECE_SIZE
      || !(self.piece_offset as usize).is_multiple_of(PIECE_SIZE)
    {
      return Err(WireError("invalid piece size or offset"));
    }
    let mut bytes = Vec::with_capacity(PIECE_HEADER + self.data.len());
    bytes.extend_from_slice(b"Q2QP\x01");
    bytes.extend_from_slice(&self.block_index.to_be_bytes());
    bytes.extend_from_slice(&self.piece_offset.to_be_bytes());
    bytes.extend_from_slice(self.data);
    Ok(bytes)
  }
}

impl<'a> Piece<'a> {
  pub fn decode(bytes: &'a [u8]) -> Result<Self, WireError> {
    if bytes.len() <= PIECE_HEADER
      || bytes.len() > PIECE_HEADER + PIECE_SIZE
      || &bytes[..5] != b"Q2QP\x01"
    {
      return Err(WireError("invalid binary piece"));
    }
    let block_index = u64::from_be_bytes(bytes[5..13].try_into().expect("checked piece header"));
    let piece_offset = u32::from_be_bytes(bytes[13..17].try_into().expect("checked piece header"));
    if !(piece_offset as usize).is_multiple_of(PIECE_SIZE) {
      return Err(WireError("unaligned piece offset"));
    }
    Ok(Self {
      block_index,
      piece_offset,
      data: &bytes[PIECE_HEADER..],
    })
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn upload_wait_has_a_bounded_duration() {
    for ms in [0, 30_001, u32::MAX] {
      let raw = Control::UploadWait {
        block_index: 7,
        retry_after: ms,
      }
      .encode()
      .unwrap();
      assert!(Control::decode(raw.as_bytes()).is_err());
    }
    for ms in [1, 2000, 30_000] {
      let raw = Control::UploadWait {
        block_index: 7,
        retry_after: ms,
      }
      .encode()
      .unwrap();
      assert!(
        matches!(Control::decode(raw.as_bytes()).unwrap(), Control::UploadWait { block_index: 7, retry_after } if retry_after == ms)
      );
    }
  }

  #[test]
  fn malformed_piece_headers_cannot_escape_message_bounds() {
    for size in 0..=PIECE_HEADER {
      assert!(Piece::decode(&vec![0; size]).is_err());
    }
    let mut encoded = Piece {
      block_index: 7,
      piece_offset: 1024,
      data: &[5; 1024],
    }
    .encode()
    .unwrap();
    assert_eq!(Piece::decode(&encoded).unwrap().block_index, 7);
    encoded[16] = 1;
    assert!(Piece::decode(&encoded).is_err());
  }

  #[test]
  fn peer_messages_reject_unknown_versions_fields_and_unbounded_ranges() {
    for raw in [
      r#"{"v":2,"message":{"type":"have","block_index":0}}"#,
      r#"{"v":1,"message":{"type":"have","block_index":0,"extra":1}}"#,
      r#"{"v":1,"message":{"type":"piece_nack","block_index":0,"piece_offset":[0],"length":[]}}"#,
    ] {
      assert!(Control::decode(raw.as_bytes()).is_err());
    }
    assert!(Control::decode(&vec![0; MAX_CONTROL_BYTES + 1]).is_err());
  }
}
