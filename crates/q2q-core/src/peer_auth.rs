//! Channel-bound peer identity proof, independent of the WebRTC runtime.

/// The roles come from the connect/offer flow, not from DTLS role negotiation.
#[derive(Clone, Copy)]
pub enum Role {
  Initiator,
  Responder,
}

/// The common transcript both peers sign after exchanging fresh challenges.
/// Keys, challenges and DTLS fingerprints are ordered by the signalling roles.
pub struct Transcript<'a> {
  pub initiator_key: [u8; 32],
  pub responder_key: [u8; 32],
  pub info_hash: [u8; 32],
  pub connection_id: &'a str,
  pub initiator_nonce: [u8; 32],
  pub responder_nonce: [u8; 32],
  pub initiator_fingerprint: [u8; 32],
  pub responder_fingerprint: [u8; 32],
}

impl Transcript<'_> {
  pub fn signing_bytes(&self, signer: Role) -> Vec<u8> {
    let mut bytes = b"q2q/peer-identity/v1\0".to_vec();
    bytes.push(match signer {
      Role::Initiator => 0,
      Role::Responder => 1,
    });
    bytes.extend_from_slice(&self.initiator_key);
    bytes.extend_from_slice(&self.responder_key);
    bytes.extend_from_slice(&self.info_hash);
    bytes.extend_from_slice(&(self.connection_id.len() as u64).to_be_bytes());
    bytes.extend_from_slice(self.connection_id.as_bytes());
    bytes.extend_from_slice(&self.initiator_nonce);
    bytes.extend_from_slice(&self.responder_nonce);
    bytes.extend_from_slice(&self.initiator_fingerprint);
    bytes.extend_from_slice(&self.responder_fingerprint);
    bytes
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::identity::{keypair_from_seed, sign_message, verify_signature};

  #[test]
  fn proof_cannot_be_replayed_across_roles_content_sessions_or_channels() {
    let (key, _) = keypair_from_seed(&[1; 32]);
    let public_key = key.verifying_key().to_bytes();
    let transcript = || Transcript {
      initiator_key: public_key,
      responder_key: [2; 32],
      info_hash: [3; 32],
      connection_id: "connection-a",
      initiator_nonce: [4; 32],
      responder_nonce: [5; 32],
      initiator_fingerprint: [6; 32],
      responder_fingerprint: [7; 32],
    };
    let signature = sign_message(&key, &transcript().signing_bytes(Role::Initiator));
    assert!(verify_signature(
      &public_key,
      &transcript().signing_bytes(Role::Initiator),
      &signature,
    ));
    assert!(!verify_signature(
      &public_key,
      &transcript().signing_bytes(Role::Responder),
      &signature,
    ));

    for field in 0..8 {
      let mut changed = transcript();
      match field {
        0 => changed.initiator_key[0] ^= 1,
        1 => changed.responder_key[0] ^= 1,
        2 => changed.info_hash[0] ^= 1,
        3 => changed.connection_id = "connection-b",
        4 => changed.initiator_nonce[0] ^= 1,
        5 => changed.responder_nonce[0] ^= 1,
        6 => changed.initiator_fingerprint[0] ^= 1,
        7 => changed.responder_fingerprint[0] ^= 1,
        _ => unreachable!(),
      }
      assert!(!verify_signature(
        &public_key,
        &changed.signing_bytes(Role::Initiator),
        &signature,
      ));
    }
  }
}
