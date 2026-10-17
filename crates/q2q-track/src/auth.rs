//! Authentication: the announce ticket, then HMAC for everything after.
//!
//! Only `announce` carries a full ticket. Once a `session_key` exists, later
//! requests authenticate with HMAC — a heartbeat every few seconds does not
//! want an Ed25519 verify plus 96 bytes of key and signature each time.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use q2q_core::identity::{Audience, PeerId, Ticket, TicketError};
// the wire form is shared with Punch, so it lives in core and is re-exported
pub use q2q_core::identity::TicketBody;

/// Short-lived tickets and clock skew both live inside this window.
pub const CLOCK_TOLERANCE: u64 = 60;

/// How far a sequence number may fall behind the last one seen. Sequence
/// numbers guard against replay, so they must go up; the window only absorbs
/// reordering in transit.
pub const SEQ_REORDER_WINDOW: u64 = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
  MalformedTicket(String),
  BadSignature,
  KeyNotIssuer,
  Expired,
  WrongAudience,
  BadMac,
  StaleTimestamp,
}

impl AuthError {
  pub fn code(&self) -> &'static str {
    match self {
      AuthError::MalformedTicket(_) => "malformed_ticket",
      AuthError::BadSignature => "bad_signature",
      AuthError::KeyNotIssuer => "key_not_issuer",
      AuthError::Expired => "expired",
      AuthError::WrongAudience => "wrong_audience",
      AuthError::BadMac => "bad_mac",
      AuthError::StaleTimestamp => "stale_timestamp",
    }
  }
}

/// Decode a wire ticket and run the four checks from spec 3.1.2.
///
/// Track is the audience it must name; there is no pre-stored mapping to lean
/// on, which is the whole point of self-signed tickets.
pub fn verify_announce(body: &TicketBody, now: u64) -> Result<Ticket, AuthError> {
  let ticket = body
    .to_ticket()
    .map_err(|e| AuthError::MalformedTicket(e.to_string()))?;
  ticket
    .verify(&Audience::Track, None, now)
    .map_err(map_ticket_err)?;
  Ok(ticket)
}

fn map_ticket_err(e: TicketError) -> AuthError {
  match e {
    TicketError::KeyDoesNotMatchIssuer => AuthError::KeyNotIssuer,
    TicketError::BadSignature => AuthError::BadSignature,
    TicketError::Expired => AuthError::Expired,
    TicketError::WrongAudience => AuthError::WrongAudience,
    TicketError::WrongTarget => AuthError::WrongAudience,
    TicketError::Malformed(d) => AuthError::MalformedTicket(d),
    // 版本不对、有效期窗口自相矛盾：都是票本身签错了，不是身份或授权的问题
    TicketError::MalformedPeerId
    | TicketError::BadKey
    | TicketError::UnsupportedVersion(_)
    | TicketError::BadValidityWindow => AuthError::MalformedTicket(e.to_string()),
  }
}

/// `HMAC-SHA-256(session_key, peer_id || seq || timestamp)`.
pub fn mac_bytes(session_key: &[u8; 32], peer_id: &PeerId, seq: u64, timestamp: u64) -> [u8; 32] {
  q2q_core::mac::sha256(
    session_key,
    &[
      peer_id.as_str().as_bytes(),
      &seq.to_be_bytes(),
      &timestamp.to_be_bytes(),
    ],
  )
}

/// Check a heartbeat-style request: clock within tolerance, sequence moving
/// forward, MAC matching. Sequence bookkeeping lives in the state layer; here
/// we only reject a sequence that is too far behind to be reordering.
pub fn verify_session_request(
  session_key: &[u8; 32],
  peer_id: &PeerId,
  seq: u64,
  timestamp: u64,
  mac: &str,
  last_seq: u64,
  now: u64,
) -> Result<(), AuthError> {
  if timestamp.abs_diff(now) > CLOCK_TOLERANCE {
    return Err(AuthError::StaleTimestamp);
  }
  if seq + SEQ_REORDER_WINDOW <= last_seq {
    return Err(AuthError::BadMac);
  }

  let expected = mac_bytes(session_key, peer_id, seq, timestamp);
  let got = B64.decode(mac).map_err(|_| AuthError::BadMac)?;
  if got != expected {
    return Err(AuthError::BadMac);
  }
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;
  use q2q_core::identity::{keypair_from_seed, peer_id_from_public_key};

  /// Helper the tests use to build a wire ticket from a real key.
  fn announce(seed: u8, aud: &str, exp: u64, now: u64) -> TicketBody {
    let (sk, peer_id) = keypair_from_seed(&[seed; 32]);
    let pk = sk.verifying_key().to_bytes();
    let mut t = Ticket {
      v: 1,
      iss: peer_id,
      public_key: pk,
      iat: now,
      exp,
      aud: Audience::from_wire(aud),
      tgt: None,
      ih: None,
      cid: None,
      sig: [0u8; 64],
    };
    t.sign(&sk);
    TicketBody {
      v: t.v,
      iss: t.iss.as_str().to_string(),
      public_key: B64.encode(t.public_key),
      iat: t.iat,
      exp: t.exp,
      aud: aud.to_string(),
      tgt: None,
      ih: None,
      cid: None,
      sig: B64.encode(t.sig),
    }
  }

  #[test]
  fn valid_ticket_passes() {
    let now = 1_000_000;
    let body = announce(1, "track", now + 60, now);
    let t = verify_announce(&body, now).unwrap();
    assert_eq!(t.iss.as_str(), body.iss);
  }

  #[test]
  fn expired_ticket_is_rejected() {
    let now = 1_000_000;
    // iat 要早于 exp，否则这是个窗口倒置的票，会在那一步就被拒
    let body = announce(2, "track", now - 1, now - 120);
    assert_eq!(verify_announce(&body, now), Err(AuthError::Expired));
  }

  #[test]
  fn tampered_signature_is_rejected() {
    let now = 1_000_000;
    let mut body = announce(3, "track", now + 60, now);
    let mut sig = B64.decode(&body.sig).unwrap();
    sig[0] ^= 0xff;
    body.sig = B64.encode(sig);
    assert_eq!(verify_announce(&body, now), Err(AuthError::BadSignature));
  }

  #[test]
  fn ticket_naming_another_audience_is_rejected() {
    let now = 1_000_000;
    // signed for one Punch Server, then presented to Track
    let body = announce(4, "punch-42", now + 60, now);
    assert_eq!(verify_announce(&body, now), Err(AuthError::WrongAudience));
  }

  #[test]
  fn public_key_not_matching_iss_is_rejected() {
    let now = 1_000_000;
    let mut body = announce(5, "track", now + 60, now);
    // a different key, same iss: the whole attack this check exists for
    body.public_key = B64.encode([9u8; 32]);
    assert_eq!(verify_announce(&body, now), Err(AuthError::KeyNotIssuer));
  }

  #[test]
  fn mac_covers_peer_seq_and_timestamp() {
    let key = [7u8; 32];
    let peer = peer_id_from_public_key(&[1u8; 32]);
    let a = mac_bytes(&key, &peer, 1, 100);
    assert_ne!(a, mac_bytes(&key, &peer, 2, 100));
    assert_ne!(a, mac_bytes(&key, &peer, 1, 101));
    assert_eq!(a, mac_bytes(&key, &peer, 1, 100));
  }

  #[test]
  fn session_request_accepts_and_rejects() {
    let key = [3u8; 32];
    let peer = peer_id_from_public_key(&[2u8; 32]);
    let now = 5_000_000;
    let good = B64.encode(mac_bytes(&key, &peer, 10, now));
    assert_eq!(
      verify_session_request(&key, &peer, 10, now, &good, 9, now),
      Ok(())
    );
    // a sequence this far behind is not reordering, it is a replay
    assert_eq!(
      verify_session_request(&key, &peer, 10, now, &good, 10 + SEQ_REORDER_WINDOW, now),
      Err(AuthError::BadMac)
    );
    // stale clock
    assert_eq!(
      verify_session_request(
        &key,
        &peer,
        11,
        now + 10_000,
        &B64.encode(mac_bytes(&key, &peer, 11, now + 10_000)),
        10,
        now
      ),
      Err(AuthError::StaleTimestamp)
    );
  }
}
