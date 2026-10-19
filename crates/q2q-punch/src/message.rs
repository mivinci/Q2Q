//! Wire messages for the binding RPCs, and the checks that go with them.
//!
//! JSON, matching Track. The transport that carries them (QUIC) belongs to
//! `q2q-punch-server` — this crate stays about the protocol and stays testable
//! without a socket.

use crate::binding::{Generation, UnixSeconds};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use q2q_primitives::identity::{Audience, TicketBody, TicketError, verify_signature};
use serde::{Deserialize, Serialize};

/// Clock tolerance for a join request, in seconds.
pub const CLOCK_TOLERANCE: UnixSeconds = 60;

// ------------------------------------------------------------------- join

/// `punch.join`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinRequest {
  /// The ticket, as a JSON string. The signature below covers exactly these
  /// bytes, so it has to be kept verbatim until the signature is checked.
  pub join_token: String,
  /// base64 of the 32-byte Ed25519 public key.
  pub public_key: String,
  /// base64 of `sign(private_key, "join" || join_token_raw || timestamp)`.
  pub signature: String,
  pub timestamp: UnixSeconds,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinResponse {
  pub binding_id: String,
  /// base64 of the 32-byte symmetric key used for later MACs.
  pub binding_key: String,
  pub expires_at: UnixSeconds,
}

/// The bytes a join signature covers.
pub fn join_signing_bytes(token_raw: &str, timestamp: UnixSeconds) -> Vec<u8> {
  let mut out = b"join".to_vec();
  out.extend_from_slice(token_raw.as_bytes());
  out.extend_from_slice(timestamp.to_string().as_bytes());
  out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinError {
  MalformedToken(String),
  MalformedKey(String),
  Ticket(TicketError),
  StaleTimestamp,
  /// The detached signature did not verify: the ticket may be valid, but the
  /// caller is not the holder of that identity.
  BadSignature,
}

impl std::fmt::Display for JoinError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      JoinError::MalformedToken(d) => write!(f, "malformed token: {d}"),
      JoinError::MalformedKey(d) => write!(f, "malformed key: {d}"),
      JoinError::Ticket(e) => write!(f, "ticket: {e}"),
      JoinError::StaleTimestamp => write!(f, "timestamp outside the clock window"),
      JoinError::BadSignature => write!(f, "signature does not match the ticket's key"),
    }
  }
}

impl std::error::Error for JoinError {}

impl JoinError {
  pub fn code(&self) -> &'static str {
    match self {
      JoinError::MalformedToken(_) | JoinError::MalformedKey(_) => "malformed_request",
      JoinError::Ticket(e) => match e {
        TicketError::Expired => "expired",
        TicketError::BadSignature => "bad_signature",
        TicketError::KeyDoesNotMatchIssuer => "key_not_issuer",
        TicketError::WrongAudience | TicketError::WrongTarget => "wrong_audience",
        _ => "malformed_ticket",
      },
      JoinError::StaleTimestamp => "stale_timestamp",
      JoinError::BadSignature => "bad_signature",
    }
  }
}

/// Verify a join: the ticket proves who the caller claims to be, the detached
/// signature proves the caller is live on the other end of this channel.
///
/// `punch_id` is this server's own id — the ticket must name it, so a captured
/// ticket cannot be spent at a different Punch Server.
pub fn verify_join(
  req: &JoinRequest,
  punch_id: &str,
  now: UnixSeconds,
) -> Result<TicketBody, JoinError> {
  if req.timestamp.abs_diff(now) > CLOCK_TOLERANCE {
    return Err(JoinError::StaleTimestamp);
  }

  let body: TicketBody = serde_json::from_str(&req.join_token)
    .map_err(|e| JoinError::MalformedToken(format!("token: {e}")))?;
  let ticket = body
    .to_ticket()
    .map_err(|e| JoinError::MalformedToken(e.to_string()))?;
  ticket
    .verify(&Audience::Punch(punch_id.to_string()), None, now)
    .map_err(JoinError::Ticket)?;

  let public_key: [u8; 32] = B64
    .decode(&req.public_key)
    .map_err(|e| JoinError::MalformedKey(format!("public_key: {e}")))?
    .try_into()
    .map_err(|_| JoinError::MalformedKey("public_key must be 32 bytes".into()))?;
  let sig: [u8; 64] = B64
    .decode(&req.signature)
    .map_err(|e| JoinError::MalformedKey(format!("signature: {e}")))?
    .try_into()
    .map_err(|_| JoinError::MalformedKey("signature must be 64 bytes".into()))?;

  // the key in the request must be the one the ticket names, otherwise the
  // detached signature proves nothing about the ticket's identity
  if public_key != ticket.public_key {
    return Err(JoinError::BadSignature);
  }
  if !verify_signature(
    &public_key,
    &join_signing_bytes(&req.join_token, req.timestamp),
    &sig,
  ) {
    return Err(JoinError::BadSignature);
  }

  Ok(body)
}

// ------------------------------------------------------- heartbeat and exit

/// `punch.heartbeat`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatRequest {
  pub binding_id: String,
  pub seq: u64,
  pub timestamp: UnixSeconds,
  pub generation: Generation,
  /// hex of `HMAC-SHA-256(binding_key, binding_id || seq || timestamp || generation)`
  pub mac: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatResponse {
  pub expires_at: UnixSeconds,
}

/// `punch.exit`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExitRequest {
  pub binding_id: String,
  pub seq: u64,
  pub timestamp: UnixSeconds,
  pub generation: Generation,
  pub mac: String,
}

/// MAC over a binding message. Note this is hex, unlike Track's base64 — the
/// spec specifies them differently and matching each is what matters.
/// The bytes a binding MAC covers. Note the encoding is hex, unlike Track's
/// base64 — the spec states them differently and matching each is what matters.
fn mac_parts(
  binding_id: &str,
  seq: u64,
  timestamp: UnixSeconds,
  generation: Generation,
) -> [Vec<u8>; 4] {
  [
    binding_id.as_bytes().to_vec(),
    seq.to_be_bytes().to_vec(),
    timestamp.to_be_bytes().to_vec(),
    generation.to_be_bytes().to_vec(),
  ]
}

pub fn binding_mac(
  key: &[u8; 32],
  binding_id: &str,
  seq: u64,
  timestamp: UnixSeconds,
  generation: Generation,
) -> String {
  let parts = mac_parts(binding_id, seq, timestamp, generation);
  let parts: Vec<&[u8]> = parts.iter().map(|p| p.as_slice()).collect();
  q2q_primitives::mac::sha256(key, &parts)
    .iter()
    .map(|b| format!("{b:02x}"))
    .collect()
}

/// Check a MAC without leaking where the first differing byte was.
pub fn verify_binding_mac(
  key: &[u8; 32],
  binding_id: &str,
  seq: u64,
  timestamp: UnixSeconds,
  generation: Generation,
  mac: &str,
) -> bool {
  let Ok(expected) = hex_decode(mac) else {
    return false;
  };
  let Ok(expected) = <[u8; 32]>::try_from(expected.as_slice()) else {
    return false;
  };
  let parts = mac_parts(binding_id, seq, timestamp, generation);
  let parts: Vec<&[u8]> = parts.iter().map(|p| p.as_slice()).collect();
  q2q_primitives::mac::verify(expected, key, &parts)
}

/// Exit credentials have a separate domain from heartbeat credentials.
pub fn exit_mac(
  key: &[u8; 32],
  binding_id: &str,
  seq: u64,
  timestamp: UnixSeconds,
  generation: Generation,
) -> String {
  let parts = mac_parts(binding_id, seq, timestamp, generation);
  let parts: Vec<&[u8]> = std::iter::once(b"punch.exit".as_slice())
    .chain(parts.iter().map(Vec::as_slice))
    .collect();
  q2q_primitives::mac::sha256(key, &parts)
    .iter()
    .map(|b| format!("{b:02x}"))
    .collect()
}

pub fn verify_exit_mac(
  key: &[u8; 32],
  binding_id: &str,
  seq: u64,
  timestamp: UnixSeconds,
  generation: Generation,
  mac: &str,
) -> bool {
  let Ok(expected) = hex_decode(mac).and_then(|bytes| bytes.try_into().map_err(|_| ())) else {
    return false;
  };
  let parts = mac_parts(binding_id, seq, timestamp, generation);
  let parts: Vec<&[u8]> = std::iter::once(b"punch.exit".as_slice())
    .chain(parts.iter().map(Vec::as_slice))
    .collect();
  q2q_primitives::mac::verify(expected, key, &parts)
}

fn hex_decode(s: &str) -> Result<Vec<u8>, ()> {
  if s.len() != 64 || !s.is_ascii() {
    return Err(());
  }
  s.as_bytes()
    .as_chunks::<2>()
    .0
    .iter()
    .map(|pair| {
      let high = (pair[0] as char).to_digit(16).ok_or(())?;
      let low = (pair[1] as char).to_digit(16).ok_or(())?;
      Ok(((high << 4) | low) as u8)
    })
    .collect()
}

// -------------------------------------------------------------------- signal

/// `punch.signal`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalResponse {
  pub ok: bool,
  /// Only ever set on the first `offer` response, like `binding_key` on join.
  #[serde(skip_serializing_if = "Option::is_none")]
  pub signal_key: Option<String>,
  /// Signalling A missed while away, replayed on resume.
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub pending: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignalType {
  Connect,
  Accept,
  Reject,
  Offer,
  Answer,
  Candidate,
  Cancel,
}

impl SignalType {
  pub fn name(self) -> &'static str {
    match self {
      Self::Connect => "connect",
      Self::Accept => "accept",
      Self::Reject => "reject",
      Self::Offer => "offer",
      Self::Answer => "answer",
      Self::Candidate => "candidate",
      Self::Cancel => "cancel",
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
  Busy,
  QuotaExceeded,
  NotFound,
  Denied,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalRequest {
  #[serde(rename = "type")]
  pub kind: SignalType,
  pub connection_id: String,
  #[serde(default)]
  pub connect_token: Option<String>,
  #[serde(default)]
  pub sdp: Option<String>,
  #[serde(default)]
  pub candidate: Option<String>,
  #[serde(default)]
  pub reason: Option<RejectReason>,
  #[serde(default)]
  pub accept_token: Option<String>,
  #[serde(default)]
  pub signal_key: Option<String>,
}

// ------------------------------------------------------------------ envelope

pub const PROTOCOL_VERSION: u32 = 1;

/// A versioned envelope. Every response carries the version, the request id and
/// — on failure — an error code, so a client can match replies to requests
/// across a transport that may reorder them.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
  pub v: u32,
  pub id: String,
  pub method: String,
  #[serde(default)]
  pub params: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RpcError {
  pub code: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Response {
  pub v: u32,
  pub id: String,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub result: Option<serde_json::Value>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub error: Option<RpcError>,
}

impl Response {
  pub fn ok(id: &str, result: &impl Serialize) -> Response {
    Response {
      v: PROTOCOL_VERSION,
      id: id.to_string(),
      result: serde_json::to_value(result).ok(),
      error: None,
    }
  }

  pub fn err(id: &str, code: &str, detail: Option<String>) -> Response {
    Response {
      v: PROTOCOL_VERSION,
      id: id.to_string(),
      result: None,
      error: Some(RpcError {
        code: code.into(),
        detail,
      }),
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
  Join,
  Heartbeat,
  Exit,
  /// Carries connect / accept / reject / offer / answer / candidate / cancel.
  Signal,
}

impl Method {
  pub fn name(self) -> &'static str {
    match self {
      Method::Join => "punch.join",
      Method::Heartbeat => "punch.heartbeat",
      Method::Exit => "punch.exit",
      Method::Signal => "punch.signal",
    }
  }

  pub fn parse(s: &str) -> Option<Method> {
    match s {
      "punch.join" => Some(Method::Join),
      "punch.heartbeat" => Some(Method::Heartbeat),
      "punch.exit" => Some(Method::Exit),
      "punch.signal" => Some(Method::Signal),
      _ => None,
    }
  }
}

/// Parse an envelope, rejecting a version we do not speak.
pub fn parse_request(bytes: &[u8]) -> Result<Request, RpcError> {
  let req: Request = serde_json::from_slice(bytes).map_err(|e| RpcError {
    code: "malformed_request".into(),
    detail: Some(e.to_string()),
  })?;
  validate_request_version(&req)?;
  Ok(req)
}

/// Validate the version of an already decoded request envelope.
///
/// Transports that wrap requests in another framing format (such as Punch's
/// WebSocket envelope) must apply the same version check as [`parse_request`].
pub fn validate_request_version(req: &Request) -> Result<(), RpcError> {
  if req.v != PROTOCOL_VERSION {
    return Err(RpcError {
      code: "unsupported_version".into(),
      detail: Some(format!("server speaks {PROTOCOL_VERSION}")),
    });
  }
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;
  use q2q_primitives::identity::{
    TICKET_VERSION, Ticket, keypair_from_seed, peer_id_of, sign_message,
  };

  /// Build a join request the way a client would.
  fn join(seed: u8, aud: &str, now: UnixSeconds) -> JoinRequest {
    let (sk, _) = keypair_from_seed(&[seed; 32]);
    let pk = sk.verifying_key().to_bytes();

    let mut ticket = Ticket {
      v: TICKET_VERSION,
      iss: peer_id_of(&sk),
      public_key: pk,
      iat: now,
      exp: now + 600,
      aud: Audience::Punch(aud.to_string()).to_wire(),
      tgt: None,
      ih: None,
      cid: None,
      sig: [0u8; 64],
    };
    ticket.sign(&sk);

    let body = TicketBody {
      v: ticket.v,
      iss: ticket.iss.as_str().to_string(),
      public_key: B64.encode(ticket.public_key),
      iat: ticket.iat,
      exp: ticket.exp,
      aud: Audience::Punch(aud.to_string()).to_wire(),
      tgt: None,
      ih: None,
      cid: None,
      sig: B64.encode(ticket.sig),
    };
    let token_raw = serde_json::to_string(&body).unwrap();
    let sig = sign_message(&sk, &join_signing_bytes(&token_raw, now));

    JoinRequest {
      join_token: token_raw,
      public_key: B64.encode(pk),
      signature: B64.encode(sig),
      timestamp: now,
    }
  }

  #[test]
  fn a_well_formed_join_passes() {
    let now = 1_000_000;
    let req = join(1, "punch-1", now);
    let body = verify_join(&req, "punch-1", now).unwrap();
    assert_eq!(body.aud, "punch:punch-1");
  }

  #[test]
  fn a_ticket_for_another_punch_is_refused() {
    let now = 1_000_000;
    let req = join(2, "punch-2", now);
    assert!(matches!(
      verify_join(&req, "punch-1", now),
      Err(JoinError::Ticket(TicketError::WrongAudience))
    ));
  }

  #[test]
  fn tampering_with_the_token_breaks_the_signature() {
    let now = 1_000_000;
    let mut req = join(3, "punch-1", now);
    req.join_token.push(' ');
    // the ticket still parses; what breaks is the detached signature, which
    // covers the token verbatim
    assert_eq!(
      verify_join(&req, "punch-1", now),
      Err(JoinError::BadSignature)
    );
  }

  #[test]
  fn a_stale_timestamp_is_refused() {
    let now = 1_000_000;
    let req = join(4, "punch-1", now);
    assert_eq!(
      verify_join(&req, "punch-1", now + 10_000),
      Err(JoinError::StaleTimestamp)
    );
  }

  #[test]
  fn a_key_that_is_not_the_tickets_is_refused() {
    let now = 1_000_000;
    let mut req = join(5, "punch-1", now);
    // sign with a different key than the ticket names
    let other = keypair_from_seed(&[99; 32]).0;
    req.public_key = B64.encode(other.verifying_key().to_bytes());
    req.signature = B64.encode(sign_message(
      &other,
      &join_signing_bytes(&req.join_token, now),
    ));
    assert_eq!(
      verify_join(&req, "punch-1", now),
      Err(JoinError::BadSignature)
    );
  }

  #[test]
  fn a_replayed_join_with_a_wrong_signature_is_refused() {
    let now = 1_000_000;
    let mut req = join(6, "punch-1", now);
    // valid ticket, but the detached signature is garbage
    let mut sig = B64.decode(&req.signature).unwrap();
    sig[0] ^= 0xff;
    req.signature = B64.encode(sig);
    assert_eq!(
      verify_join(&req, "punch-1", now),
      Err(JoinError::BadSignature)
    );
  }

  #[test]
  fn mac_covers_every_field() {
    let key = [7u8; 32];
    let base = binding_mac(&key, "b1", 1, 100, 0);
    assert_ne!(base, binding_mac(&key, "b2", 1, 100, 0));
    assert_ne!(base, binding_mac(&key, "b1", 2, 100, 0));
    assert_ne!(base, binding_mac(&key, "b1", 1, 101, 0));
    assert_ne!(base, binding_mac(&key, "b1", 1, 100, 1));
    assert_eq!(base, binding_mac(&key, "b1", 1, 100, 0));
  }

  #[test]
  fn mac_is_lowercase_hex() {
    let mac = binding_mac(&[0u8; 32], "b1", 1, 1, 0);
    assert_eq!(mac.len(), 64);
    assert!(
      mac
        .chars()
        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );
  }

  #[test]
  fn mac_verification_is_exact() {
    let key = [3u8; 32];
    let good = binding_mac(&key, "b1", 7, 100, 2);
    assert!(verify_binding_mac(&key, "b1", 7, 100, 2, &good));
    assert!(!verify_binding_mac(&key, "b1", 7, 100, 3, &good));
    assert!(!verify_binding_mac(&key, "b1", 7, 100, 2, "zzzz"));
    assert!(!verify_binding_mac(&key, "b1", 7, 100, 2, ""));
  }

  #[test]
  fn envelope_round_trips_and_keeps_the_request_id() {
    let raw = r#"{"v":1,"id":"r-1","method":"punch.exit","params":{"binding_id":"b1"}}"#;
    let req = parse_request(raw.as_bytes()).unwrap();
    assert_eq!(Method::parse(&req.method), Some(Method::Exit));
    assert_eq!(req.id, "r-1");

    let resp = Response::ok(&req.id, &HeartbeatResponse { expires_at: 42 });
    let back: Response = serde_json::from_slice(&serde_json::to_vec(&resp).unwrap()).unwrap();
    assert_eq!(back.id, "r-1");
    assert_eq!(back.error, None);
  }

  #[test]
  fn envelope_rejects_another_version_and_unknown_methods() {
    let raw = r#"{"v":2,"id":"r-1","method":"punch.join","params":{}}"#;
    let e = parse_request(raw.as_bytes()).unwrap_err();
    assert_eq!(e.code, "unsupported_version");

    let raw = r#"{"v":1,"id":"r-1","method":"punch.nope","params":{}}"#;
    // parses fine — the method is unknown, but that is the dispatcher's call
    let req = parse_request(raw.as_bytes()).unwrap();
    assert_eq!(Method::parse(&req.method), None);
  }
  #[test]
  fn malformed_mac_strings_are_rejected_without_panicking() {
    for mac in [
      "aéa".to_string(),
      "é".repeat(32),
      "💥".repeat(16),
      "z".repeat(64),
      "0".repeat(63),
    ] {
      assert!(!verify_binding_mac(&[1; 32], "binding", 1, 100, 0, &mac));
      assert!(!verify_exit_mac(&[1; 32], "binding", 1, 100, 0, &mac));
    }
  }

  #[test]
  fn a_heartbeat_mac_cannot_authorize_exit() {
    let heartbeat = binding_mac(&[1; 32], "binding", 1, 100, 0);
    let exit = exit_mac(&[1; 32], "binding", 1, 100, 0);
    assert_ne!(heartbeat, exit);
    assert!(!verify_exit_mac(&[1; 32], "binding", 1, 100, 0, &heartbeat));
    assert!(verify_exit_mac(&[1; 32], "binding", 1, 100, 0, &exit));
    assert!(!verify_binding_mac(&[1; 32], "binding", 1, 100, 0, &exit));
  }
}
