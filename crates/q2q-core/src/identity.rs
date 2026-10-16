//! Identity: Ed25519 keys, `peer_id` derivation, self-signed tickets.
//!
//! There is no registration step anywhere. A key pair *is* the identity; Track
//! and Punch only ever see a public key inside a ticket the client signed
//! itself.

use crate::bencode::{self, BValue, Dict};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

/// RFC 4648 alphabet (A–Z then 2–7), used for the data part of a `peer_id`.
const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
/// Bech32 charset, used for the six checksum characters only.
const BECH32_CHARSET: &[u8; 32] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";

pub const PEER_ID_LEN: usize = 58; // 52 data chars + 6 checksum chars

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PeerId(String);

impl PeerId {
  pub fn as_str(&self) -> &str {
    &self.0
  }

  /// Recompute the checksum and compare. Call this on any `peer_id` that came
  /// from the network — it catches transcription errors before they turn into a
  /// lookup against the wrong identity.
  pub fn checksum_is_valid(&self) -> bool {
    let bytes = self.0.as_bytes();
    if bytes.len() != PEER_ID_LEN {
      return false;
    }
    let data = &bytes[..52];
    let mut values = Vec::with_capacity(52);
    for &c in data {
      match base32_value(c) {
        Some(v) => values.push(v),
        None => return false,
      }
    }
    let expected = checksum(&values);
    bytes[52..]
      .iter()
      .zip(expected.iter())
      .all(|(c, v)| *c == BECH32_CHARSET[*v as usize])
  }
}

impl std::fmt::Display for PeerId {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.write_str(&self.0)
  }
}

/// `base32(SHA-256(public_key))` plus a six character bech32 checksum.
pub fn peer_id_from_public_key(public_key: &[u8; 32]) -> PeerId {
  let hash = Sha256::digest(public_key);
  let data = base32_encode(&hash);
  let values: Vec<u8> = data.bytes().map(|c| base32_value(c).unwrap()).collect();
  let mut out = data;
  for v in checksum(&values) {
    out.push(BECH32_CHARSET[v as usize] as char);
  }
  PeerId(out)
}

fn base32_encode(data: &[u8]) -> String {
  let mut out = String::new();
  let mut bits = 0u32;
  let mut nbits = 0;
  for &b in data {
    bits = (bits << 8) | b as u32;
    nbits += 8;
    while nbits >= 5 {
      nbits -= 5;
      out.push(BASE32_ALPHABET[((bits >> nbits) & 31) as usize] as char);
    }
  }
  if nbits > 0 {
    // 32 bytes leave a single bit over; pad it with zeros, no '=' padding
    out.push(BASE32_ALPHABET[((bits << (5 - nbits)) & 31) as usize] as char);
  }
  out
}

fn base32_value(c: u8) -> Option<u8> {
  BASE32_ALPHABET
    .iter()
    .position(|&a| a == c)
    .map(|i| i as u8)
}

/// BIP-173 polymod. The hrp is empty: a `peer_id` is not a prefixed bech32
/// address, so only the data participates.
fn checksum(data: &[u8]) -> [u8; 6] {
  const GEN: [u32; 5] = [
    0x3b6a_57b2,
    0x2650_8e6d,
    0x1ea1_19fa,
    0x3d42_33dd,
    0x2a14_62b3,
  ];
  let mut chk: u32 = 1;
  // empty hrp contributes a single zero
  for v in std::iter::once(&0u8).chain(data).chain(&[0u8; 6]) {
    let b = chk >> 25;
    chk = (chk & 0x01ff_ffff) << 5 ^ (*v as u32);
    for (i, &gen) in GEN.iter().enumerate() {
      if (b >> i) & 1 == 1 {
        chk ^= gen;
      }
    }
  }
  let chk = chk ^ 1;
  let mut out = [0u8; 6];
  for (i, o) in out.iter_mut().enumerate() {
    *o = ((chk >> (5 * (5 - i))) & 31) as u8;
  }
  out
}

/// Who a ticket may be presented to. For Punch this is the concrete server id,
/// not a literal: a leaked ticket cannot be replayed at another Punch node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Audience {
  Track,
  Punch(String),
}

impl Audience {
  fn as_bytes(&self) -> Vec<u8> {
    match self {
      Audience::Track => b"track".to_vec(),
      Audience::Punch(id) => id.as_bytes().to_vec(),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ticket {
  pub v: u32,
  pub iss: PeerId,
  /// Must be embedded: `peer_id` is a hash, the public key cannot be derived
  /// back from it.
  pub public_key: [u8; 32],
  pub iat: u64,
  pub exp: u64,
  pub aud: Audience,
  pub tgt: Option<PeerId>,
  pub ih: Option<[u8; 32]>,
  pub cid: Option<String>,
  pub sig: [u8; 64],
}

impl Ticket {
  /// The signed payload. Absent optional fields are omitted rather than encoded
  /// empty — otherwise the same ticket could have two different signatures.
  fn payload(&self) -> BValue {
    let mut d = Dict::new();
    d.insert(b"v".to_vec(), BValue::Int(self.v as i64));
    d.insert(b"iss".to_vec(), BValue::bytes(self.iss.as_str().as_bytes()));
    d.insert(b"pub".to_vec(), BValue::Bytes(self.public_key.to_vec()));
    d.insert(b"iat".to_vec(), BValue::Int(self.iat as i64));
    d.insert(b"exp".to_vec(), BValue::Int(self.exp as i64));
    d.insert(b"aud".to_vec(), BValue::Bytes(self.aud.as_bytes()));
    if let Some(tgt) = &self.tgt {
      d.insert(b"tgt".to_vec(), BValue::bytes(tgt.as_str().as_bytes()));
    }
    if let Some(ih) = &self.ih {
      d.insert(b"ih".to_vec(), BValue::Bytes(ih.to_vec()));
    }
    if let Some(cid) = &self.cid {
      d.insert(b"cid".to_vec(), BValue::bytes(cid.as_bytes()));
    }
    BValue::Dict(d)
  }

  pub fn sign(&mut self, key: &SigningKey) {
    self.sig = key.sign(&self.payload().encode()).to_bytes();
  }

  /// The four checks of spec 3.1.2, in order. `expected_audience` is the
  /// verifier's own identity and `me` is its `peer_id` when it is also the
  /// connection target.
  pub fn verify(
    &self,
    expected_audience: &Audience,
    me: Option<&PeerId>,
    now: u64,
  ) -> Result<(), TicketError> {
    // 1. the embedded key must actually be the issuer's
    if peer_id_from_public_key(&self.public_key) != self.iss {
      return Err(TicketError::KeyDoesNotMatchIssuer);
    }
    if !self.iss.checksum_is_valid() {
      return Err(TicketError::MalformedPeerId);
    }

    // 2. possession of the private key
    let verifying = VerifyingKey::from_bytes(&self.public_key).map_err(|_| TicketError::BadKey)?;
    let sig = Signature::from_bytes(&self.sig);
    verifying
      .verify(&self.payload().encode(), &sig)
      .map_err(|_| TicketError::BadSignature)?;

    // 3. still valid
    if now >= self.exp {
      return Err(TicketError::Expired);
    }

    // 4. this ticket was meant for us
    if &self.aud != expected_audience {
      return Err(TicketError::WrongAudience);
    }
    if let Some(tgt) = &self.tgt {
      match me {
        Some(me) if me == tgt => {}
        _ => return Err(TicketError::WrongTarget),
      }
    }
    Ok(())
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TicketError {
  MalformedPeerId,
  KeyDoesNotMatchIssuer,
  BadKey,
  BadSignature,
  Expired,
  WrongAudience,
  WrongTarget,
}

impl std::fmt::Display for TicketError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    let s = match self {
      TicketError::MalformedPeerId => "peer_id fails its checksum",
      TicketError::KeyDoesNotMatchIssuer => "embedded public key does not derive iss",
      TicketError::BadKey => "public key is not a valid Ed25519 key",
      TicketError::BadSignature => "signature does not verify",
      TicketError::Expired => "ticket has expired",
      TicketError::WrongAudience => "ticket was issued to a different audience",
      TicketError::WrongTarget => "ticket names a different target peer",
    };
    f.write_str(s)
  }
}

impl std::error::Error for TicketError {}

/// Generate a key pair from the system CSPRNG. The caller owns persistence;
/// the private key never leaves the device.
pub fn generate_keypair() -> Result<(SigningKey, PeerId), getrandom::Error> {
  let mut seed = [0u8; 32];
  getrandom::fill(&mut seed)?;
  Ok(keypair_from_seed(&seed))
}

/// Deterministic construction, for tests and for restoring a stored seed.
pub fn keypair_from_seed(seed: &[u8; 32]) -> (SigningKey, PeerId) {
  let sk = SigningKey::from_bytes(seed);
  let id = peer_id_from_public_key(&sk.verifying_key().to_bytes());
  (sk, id)
}

pub fn public_key_of(sk: &SigningKey) -> [u8; 32] {
  sk.verifying_key().to_bytes()
}

pub fn peer_id_of(sk: &SigningKey) -> PeerId {
  peer_id_from_public_key(&public_key_of(sk))
}

pub fn digest_payload(t: &Ticket) -> [u8; 32] {
  bencode::digest(&t.payload())
}

#[cfg(test)]
mod tests {
  use super::*;

  fn key_from(seed: u8) -> SigningKey {
    keypair_from_seed(&[seed; 32]).0
  }

  #[test]
  fn generated_keys_are_independent() {
    let (a, id_a) = generate_keypair().unwrap();
    let (b, id_b) = generate_keypair().unwrap();
    assert_ne!(id_a, id_b);
    assert_ne!(public_key_of(&a), public_key_of(&b));
  }

  fn ticket_from(sk: &SigningKey, now: u64) -> Ticket {
    let mut t = Ticket {
      v: 1,
      iss: peer_id_of(sk),
      public_key: public_key_of(sk),
      iat: now,
      exp: now + 60,
      aud: Audience::Track,
      tgt: None,
      ih: None,
      cid: None,
      sig: [0u8; 64],
    };
    t.sign(sk);
    t
  }

  #[test]
  fn peer_id_has_expected_shape() {
    let sk = key_from(7);
    let id = peer_id_of(&sk);
    assert_eq!(id.as_str().len(), PEER_ID_LEN);
    assert!(id.checksum_is_valid());
  }

  #[test]
  fn peer_id_is_derived_from_the_key() {
    let a = key_from(1);
    let b = key_from(2);
    assert_ne!(peer_id_of(&a), peer_id_of(&b));
    // same key always gives the same id
    assert_eq!(peer_id_of(&a), peer_id_of(&a));
  }

  #[test]
  fn checksum_detects_single_character_errors() {
    let sk = key_from(3);
    let id = peer_id_of(&sk);
    let mut chars: Vec<char> = id.as_str().chars().collect();
    for i in [0usize, 10, 30, 51, 57] {
      let original = chars[i];
      chars[i] = if original == 'A' { 'B' } else { 'A' };
      let bad = PeerId(chars.iter().collect());
      assert!(!bad.checksum_is_valid(), "position {i} not detected");
      chars[i] = original;
    }
  }

  #[test]
  fn roundtrip_ticket() {
    let sk = key_from(4);
    let t = ticket_from(&sk, 1000);
    assert_eq!(t.verify(&Audience::Track, None, 1000), Ok(()));
  }

  #[test]
  fn rejects_key_that_is_not_the_issuer() {
    // sign with one key but claim another peer's id
    let sk = key_from(5);
    let other = key_from(6);
    let mut t = ticket_from(&sk, 1000);
    t.iss = peer_id_of(&other);
    assert_eq!(
      t.verify(&Audience::Track, None, 1000),
      Err(TicketError::KeyDoesNotMatchIssuer)
    );
  }

  #[test]
  fn rejects_expired() {
    let sk = key_from(7);
    let t = ticket_from(&sk, 1000);
    assert_eq!(
      t.verify(&Audience::Track, None, 5000),
      Err(TicketError::Expired)
    );
  }

  #[test]
  fn rejects_wrong_audience() {
    let sk = key_from(8);
    let t = ticket_from(&sk, 1000);
    assert_eq!(
      t.verify(&Audience::Punch("punch-1".into()), None, 1000),
      Err(TicketError::WrongAudience)
    );
  }

  #[test]
  fn punch_audience_is_the_concrete_server() {
    let sk = key_from(9);
    let mut t = ticket_from(&sk, 1000);
    t.aud = Audience::Punch("punch-42".into());
    t.sign(&sk);
    assert_eq!(
      t.verify(&Audience::Punch("punch-42".into()), None, 1000),
      Ok(())
    );
    // a different node must refuse it
    assert_eq!(
      t.verify(&Audience::Punch("punch-43".into()), None, 1000),
      Err(TicketError::WrongAudience)
    );
  }

  #[test]
  fn rejects_wrong_target() {
    let sk = key_from(10);
    let me = key_from(11);
    let mut t = ticket_from(&sk, 1000);
    t.tgt = Some(peer_id_of(&me));
    t.sign(&sk);
    assert_eq!(
      t.verify(&Audience::Track, Some(&peer_id_of(&me)), 1000),
      Ok(())
    );

    let stranger = peer_id_of(&key_from(12));
    assert_eq!(
      t.verify(&Audience::Track, Some(&stranger), 1000),
      Err(TicketError::WrongTarget)
    );
  }

  #[test]
  fn rejects_tampered_signature() {
    let sk = key_from(13);
    let mut t = ticket_from(&sk, 1000);
    t.sig[0] ^= 0xff;
    assert_eq!(
      t.verify(&Audience::Track, None, 1000),
      Err(TicketError::BadSignature)
    );
  }

  #[test]
  fn rejects_tampered_payload() {
    let sk = key_from(14);
    let mut t = ticket_from(&sk, 1000);
    t.exp = 999_999; // re-signing is what a real attacker cannot do
    assert_eq!(
      t.verify(&Audience::Track, None, 1000),
      Err(TicketError::BadSignature)
    );
  }

  #[test]
  fn payload_encoding_is_stable() {
    let sk = key_from(15);
    let t = ticket_from(&sk, 1000);
    assert_eq!(digest_payload(&t), digest_payload(&t));
  }
}
