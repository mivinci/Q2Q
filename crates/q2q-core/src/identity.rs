//! Identity: Ed25519 keys, `peer_id` derivation, self-signed tickets.
//!
//! There is no registration step anywhere. A key pair *is* the identity; Track
//! and Punch only ever see a public key inside a ticket the client signed
//! itself.

use crate::bencode::{self, BValue};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::Serialize;
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

  /// Wrap a `peer_id` taken from the wire. The checksum is not checked here;
  /// `Ticket::verify` does it, so a caller that verifies does not pay twice.
  pub fn unchecked(s: impl Into<String>) -> Self {
    PeerId(s.into())
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
  // keep the 5-bit values rather than re-decoding the string we just built
  let values = base32_values(&hash);
  let mut out = base32_string(&values);
  for v in checksum(&values) {
    out.push(BECH32_CHARSET[v as usize] as char);
  }
  PeerId(out)
}

/// Five-bit values, which is what the checksum works on.
fn base32_values(data: &[u8]) -> Vec<u8> {
  let mut out = Vec::new();
  let mut bits = 0u32;
  let mut nbits = 0;
  for &b in data {
    bits = (bits << 8) | b as u32;
    nbits += 8;
    while nbits >= 5 {
      nbits -= 5;
      out.push(((bits >> nbits) & 31) as u8);
    }
  }
  if nbits > 0 {
    // 32 bytes leave a single bit over; pad it with zeros, no '=' padding
    out.push(((bits << (5 - nbits)) & 31) as u8);
  }
  out
}

fn base32_string(values: &[u8]) -> String {
  values
    .iter()
    .map(|&v| BASE32_ALPHABET[v as usize] as char)
    .collect()
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

/// A ticket as it appears on the wire.
///
/// The spec never spells out how a ticket is encoded inside a request body, so
/// this is the shared answer: a JSON object with binary fields in base64. Both
/// The only ticket version this builds or accepts. Bump it when the payload
/// shape changes — an old ticket must stop verifying, not be quietly tolerated.
pub const TICKET_VERSION: u32 = 1;

/// Track and Punch parse tickets, so it lives here rather than in either.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TicketBody {
  pub v: u32,
  pub iss: String,
  /// base64 of the 32-byte public key — `peer_id` is a hash of it and cannot be
  /// reversed, so it has to travel along.
  #[serde(rename = "pub")]
  pub public_key: String,
  pub iat: u64,
  pub exp: u64,
  pub aud: String,
  #[serde(default)]
  pub tgt: Option<String>,
  #[serde(default)]
  pub ih: Option<String>,
  #[serde(default)]
  pub cid: Option<String>,
  /// base64 of the 64-byte signature over the canonical payload.
  pub sig: String,
}

impl TicketBody {
  /// Decode into a `Ticket` ready for `verify`. Only the encoding is checked
  /// here; correctness is `verify`'s job.
  pub fn to_ticket(&self) -> Result<Ticket, TicketError> {
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine as _;

    let public_key: [u8; 32] = B64
      .decode(&self.public_key)
      .map_err(|e| TicketError::Malformed(format!("pub: {e}")))?
      .try_into()
      .map_err(|_| TicketError::Malformed("pub must be 32 bytes".into()))?;
    let sig: [u8; 64] = B64
      .decode(&self.sig)
      .map_err(|e| TicketError::Malformed(format!("sig: {e}")))?
      .try_into()
      .map_err(|_| TicketError::Malformed("sig must be 64 bytes".into()))?;
    let ih = match &self.ih {
      Some(s) => Some(
        B64
          .decode(s)
          .map_err(|e| TicketError::Malformed(format!("ih: {e}")))?
          .try_into()
          .map_err(|_| TicketError::Malformed("ih must be 32 bytes".into()))?,
      ),
      None => None,
    };

    Ok(Ticket {
      v: self.v,
      iss: PeerId::unchecked(self.iss.clone()),
      public_key,
      iat: self.iat,
      exp: self.exp,
      aud: Audience::from_wire(&self.aud),
      tgt: self.tgt.as_ref().map(|t| PeerId::unchecked(t.clone())),
      ih,
      cid: self.cid.clone(),
      sig,
    })
  }

  /// Decode and run the four checks of spec 3.1.2.
  pub fn verify(
    &self,
    audience: &Audience,
    me: Option<&PeerId>,
    now: u64,
  ) -> Result<(), TicketError> {
    self.to_ticket()?.verify(audience, me, now)
  }
}

impl Audience {
  fn as_str(&self) -> &str {
    match self {
      Audience::Track => "track",
      Audience::Punch(id) => id.as_str(),
    }
  }

  /// Parse `aud` as it appears on the wire: either the literal `track`, or the
  /// id of the one Punch Server the ticket may be spent at.
  pub fn from_wire(s: &str) -> Self {
    match s {
      "track" => Audience::Track,
      other => Audience::Punch(other.to_string()),
    }
  }
}

/// The part of a ticket that gets signed. `sig` is deliberately absent, and the
/// key order is fixed by the serialiser rather than by declaration order.
#[derive(Serialize)]
struct Payload<'a> {
  v: u32,
  iss: &'a str,
  #[serde(rename = "pub", with = "serde_bytes")]
  public_key: &'a [u8],
  iat: u64,
  exp: u64,
  aud: &'a str,
  #[serde(skip_serializing_if = "Option::is_none")]
  tgt: Option<&'a str>,
  #[serde(skip_serializing_if = "Option::is_none", with = "serde_bytes")]
  ih: Option<&'a [u8]>,
  #[serde(skip_serializing_if = "Option::is_none")]
  cid: Option<&'a str>,
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
    let p = Payload {
      v: self.v,
      iss: self.iss.as_str(),
      public_key: &self.public_key,
      iat: self.iat,
      exp: self.exp,
      aud: self.aud.as_str(),
      tgt: self.tgt.as_ref().map(|t| t.as_str()),
      ih: self.ih.as_ref().map(|h| h.as_slice()),
      cid: self.cid.as_deref(),
    };
    crate::bencode_serde::to_value(&p).expect("ticket payload is always serialisable")
  }

  pub fn sign(&mut self, key: &SigningKey) {
    self.sig = key.sign(&self.payload().encode()).to_bytes();
  }

  /// The other direction: a signed ticket back into its wire form.
  pub fn to_body(&self) -> TicketBody {
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine as _;

    TicketBody {
      v: self.v,
      iss: self.iss.as_str().to_string(),
      public_key: B64.encode(self.public_key),
      iat: self.iat,
      exp: self.exp,
      aud: self.aud.as_str().to_string(),
      tgt: self.tgt.as_ref().map(|t| t.as_str().to_string()),
      ih: self.ih.map(|h| B64.encode(h)),
      cid: self.cid.clone(),
      sig: B64.encode(self.sig),
    }
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
    self.check_identity(now)?;

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

  /// The same checks for a verifier that is the audience but *not* the target —
  /// a Punch Server relaying a connect. Matching `tgt` against its own id would
  /// reject every valid connect; the target is recorded on the connection and
  /// checked there instead. A connect that names no target is refused outright.
  pub fn verify_as_relay(&self, expected_audience: &Audience, now: u64) -> Result<(), TicketError> {
    self.check_identity(now)?;
    if &self.aud != expected_audience {
      return Err(TicketError::WrongAudience);
    }
    if self.tgt.is_none() {
      return Err(TicketError::WrongTarget);
    }
    Ok(())
  }

  /// Steps 1–3: the key is the issuer's, the signature holds, it has not expired.
  fn check_identity(&self, now: u64) -> Result<(), TicketError> {
    // 0. before the expensive checks: the version has to be one we speak, and
    // the window has to make sense at all. spec 3.1.2's four steps assume both.
    if self.v != TICKET_VERSION {
      return Err(TicketError::UnsupportedVersion(self.v));
    }
    if self.iat > self.exp {
      return Err(TicketError::BadValidityWindow);
    }

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
    Ok(())
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TicketError {
  MalformedPeerId,
  /// The wire form could not be decoded — bad base64, wrong field length.
  Malformed(String),
  KeyDoesNotMatchIssuer,
  BadKey,
  BadSignature,
  Expired,
  WrongAudience,
  WrongTarget,
  UnsupportedVersion(u32),
  /// `iat` 晚于 `exp`——有效期窗口自相矛盾，签出来就是废的
  BadValidityWindow,
}

impl std::fmt::Display for TicketError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    let s = match self {
      TicketError::MalformedPeerId => "peer_id fails its checksum",
      TicketError::Malformed(d) => return write!(f, "malformed ticket: {d}"),
      TicketError::KeyDoesNotMatchIssuer => "embedded public key does not derive iss",
      TicketError::BadKey => "public key is not a valid Ed25519 key",
      TicketError::BadSignature => "signature does not verify",
      TicketError::Expired => "ticket has expired",
      TicketError::WrongAudience => "ticket was issued to a different audience",
      TicketError::WrongTarget => "ticket names a different target peer",
      TicketError::UnsupportedVersion(v) => {
        return write!(f, "ticket version {v} is not {TICKET_VERSION}")
      }
      TicketError::BadValidityWindow => "ticket expires before it was issued",
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

/// Verify a detached signature over an arbitrary message, made by the holder of
/// the identity this public key belongs to.
///
/// Punch needs this for `join`, which signs `"join" || token || timestamp` in
/// addition to carrying the ticket: the ticket proves who you claim to be, the
/// detached signature proves you are on the other end of this channel right now.
pub fn verify_signature(public_key: &[u8; 32], message: &[u8], sig: &[u8; 64]) -> bool {
  let Ok(verifying) = VerifyingKey::from_bytes(public_key) else {
    return false;
  };
  verifying
    .verify(message, &Signature::from_bytes(sig))
    .is_ok()
}

/// Sign an arbitrary message. Counterpart of `verify_signature`, for callers
/// that need a detached signature rather than the one inside a ticket.
pub fn sign_message(sk: &SigningKey, message: &[u8]) -> [u8; 64] {
  sk.sign(message).to_bytes()
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
  fn rejects_a_ticket_of_another_version() {
    let sk = key_from(3);
    let mut t = ticket_from(&sk, 1_000);
    // 重新签名，让它在版本之外都合法——否则只会撞到 BadSignature
    t.v = TICKET_VERSION + 1;
    t.sign(&sk);
    assert_eq!(
      t.verify(&Audience::Track, None, 1_000),
      Err(TicketError::UnsupportedVersion(TICKET_VERSION + 1))
    );
  }

  #[test]
  fn rejects_a_window_that_expires_before_it_was_issued() {
    let sk = key_from(4);
    let mut t = ticket_from(&sk, 1_000);
    t.iat = t.exp + 1;
    t.sign(&sk);
    assert_eq!(
      t.verify(&Audience::Track, None, 1_000),
      Err(TicketError::BadValidityWindow)
    );
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
