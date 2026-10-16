//! Signalling, and the state machine that makes "consent before offer" real.
//!
//! The ordering is the whole point: B answers `connect` with `accept` or
//! `reject` *before* A ever builds an SDP. A rejection therefore costs no ICE
//! gathering, no STUN query, no relay allocation. So `offer` is refused unless
//! the connection has already been accepted — that check is the reason this
//! module exists.

use crate::binding::UnixSeconds;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use parking_lot::Mutex;
use q2q_core::identity::{Audience, PeerId, TicketBody, TicketError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
      SignalType::Connect => "connect",
      SignalType::Accept => "accept",
      SignalType::Reject => "reject",
      SignalType::Offer => "offer",
      SignalType::Answer => "answer",
      SignalType::Candidate => "candidate",
      SignalType::Cancel => "cancel",
    }
  }
}

/// What the spec calls `reject.reason`. It decides whether A tries another peer,
/// re-queries Track, or gives up — so it has to survive the trip verbatim.
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
  /// Required on `connect` and `offer`. Everything else rides on the channel,
  /// which is itself the credential.
  #[serde(default)]
  pub connect_token: Option<String>,
  /// Opaque to the server: it must never parse SDP.
  #[serde(default)]
  pub sdp: Option<String>,
  #[serde(default)]
  pub candidate: Option<String>,
  #[serde(default)]
  pub reason: Option<RejectReason>,
  /// B's own ticket, so A can verify the accept without trusting the server.
  #[serde(default)]
  pub accept_token: Option<String>,
  /// Carried when A reconnects onto a live connection.
  #[serde(default)]
  pub signal_key: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
  /// Registered by `connect`, waiting on B's answer. No SDP yet.
  Pending,
  Accepted,
  /// An offer has been relayed; `signal_key` is minted here.
  Offered,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
  pub connection_id: String,
  pub a: PeerId,
  pub a_public_key: [u8; 32],
  pub b: PeerId,
  pub info_hash: Option<[u8; 32]>,
  pub state: ConnectionState,
  /// Minted with the first offer, and the only thing A needs to resume.
  pub signal_key: Option<[u8; 32]>,
  /// The accepted offer, so a retransmission is idempotent.
  pub offer: Option<String>,
  /// Signalling that reached us while A was away; replayed on resume.
  pub pending: Vec<String>,
  pub expires_at: UnixSeconds,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignalError {
  /// Also what an unauthorised caller gets: whether the connection exists is
  /// not something we tell strangers.
  UnknownConnection,
  Ticket(TicketError),
  /// The ticket names a different target.
  NotTheTarget,
  /// `offer` arrived before B accepted.
  NotAccepted,
  NotAParticipant,
  /// Same connection id, different offer: either a bug or an attack.
  OfferConflict,
  Expired,
  BadSignalKey,
  /// Nothing about the request was wrong; we could not mint a key.
  Internal,
  MissingTicket,
  /// B has no binding — the standing channel is what makes it reachable.
  PeerOffline,
}

impl SignalError {
  pub fn code(&self) -> &'static str {
    match self {
      SignalError::UnknownConnection => "unknown_connection",
      SignalError::Ticket(e) => match e {
        TicketError::Expired => "expired",
        TicketError::BadSignature => "bad_signature",
        TicketError::KeyDoesNotMatchIssuer => "key_not_issuer",
        TicketError::WrongAudience | TicketError::WrongTarget => "unauthorized",
        _ => "malformed_ticket",
      },
      SignalError::NotTheTarget => "unauthorized",
      SignalError::NotAccepted => "not_accepted",
      SignalError::NotAParticipant => "unauthorized",
      SignalError::OfferConflict => "offer_conflict",
      SignalError::Expired => "expired",
      SignalError::BadSignalKey => "unauthorized",
      SignalError::MissingTicket => "malformed_request",
      SignalError::PeerOffline => "peer_offline",
      SignalError::Internal => "internal_error",
    }
  }
}

impl std::fmt::Display for SignalError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      SignalError::UnknownConnection => write!(f, "no such connection"),
      SignalError::Ticket(e) => write!(f, "ticket: {e}"),
      SignalError::NotTheTarget => write!(f, "connection names another target"),
      SignalError::NotAccepted => write!(f, "offer before the target accepted"),
      SignalError::NotAParticipant => write!(f, "not a participant of this connection"),
      SignalError::OfferConflict => write!(f, "connection id reused with a different offer"),
      SignalError::Expired => write!(f, "connection expired"),
      SignalError::BadSignalKey => write!(f, "signal key does not match"),
      SignalError::MissingTicket => write!(f, "connect and offer need a token"),
      SignalError::PeerOffline => write!(f, "target has no binding"),
      SignalError::Internal => write!(f, "could not mint a key"),
    }
  }
}

impl std::error::Error for SignalError {}

// ---------------------------------------------------------------- verification

/// Check a connect ticket: it must name this Punch Server, name a target, name
/// the content, and not be stale.
pub fn verify_connect(
  token: &str,
  punch_id: &str,
  now: UnixSeconds,
) -> Result<TicketBody, SignalError> {
  let body: TicketBody = serde_json::from_str(token)
    .map_err(|_| SignalError::Ticket(TicketError::Malformed("token".into())))?;
  let ticket = body.to_ticket().map_err(SignalError::Ticket)?;
  // the server is the audience but not the target: it relays, so `tgt` is
  // checked against the connection rather than against itself
  ticket
    .verify_as_relay(&Audience::Punch(punch_id.to_string()), now)
    .map_err(SignalError::Ticket)?;

  Ok(body)
}

/// The content a connect ticket names, decoded.
pub fn info_hash_of(body: &TicketBody) -> Result<Option<[u8; 32]>, SignalError> {
  match &body.ih {
    Some(s) => {
      let bytes = B64
        .decode(s)
        .map_err(|_| SignalError::Ticket(TicketError::Malformed("ih".into())))?;
      let hash: [u8; 32] = bytes
        .try_into()
        .map_err(|_| SignalError::Ticket(TicketError::Malformed("ih must be 32 bytes".into())))?;
      Ok(Some(hash))
    }
    None => Ok(None),
  }
}

// -------------------------------------------------------------------- registry

pub struct Connections {
  map: Mutex<HashMap<String, Connection>>,
  /// One-shot connect ticket ids.
  spent: Mutex<HashMap<String, UnixSeconds>>,
  ttl: UnixSeconds,
}

impl Connections {
  pub fn new(ttl: UnixSeconds) -> Self {
    Connections {
      map: Mutex::new(HashMap::new()),
      spent: Mutex::new(HashMap::new()),
      ttl,
    }
  }

  /// Register a connect. The ticket id is consumed here, so the same connect
  /// cannot be opened twice.
  pub fn open(
    &self,
    ticket: &TicketBody,
    target: &PeerId,
    target_online: bool,
    now: UnixSeconds,
  ) -> Result<Connection, SignalError> {
    let cid = ticket.cid.clone().ok_or(SignalError::MissingTicket)?;
    if !target_online {
      return Err(SignalError::PeerOffline);
    }

    let a = PeerId::unchecked(&ticket.iss);
    let public_key: [u8; 32] = B64
      .decode(&ticket.public_key)
      .ok()
      .and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok())
      .ok_or(SignalError::Ticket(TicketError::Malformed("pub".into())))?;
    let named = ticket
      .tgt
      .as_ref()
      .map(PeerId::unchecked)
      .ok_or(SignalError::NotTheTarget)?;
    if &named != target {
      return Err(SignalError::NotTheTarget);
    }

    let mut spent = self.spent.lock();
    if spent.get(&cid).is_some_and(|until| *until > now) {
      return Err(SignalError::UnknownConnection);
    }
    spent.insert(cid.clone(), now + self.ttl * 2);
    drop(spent);

    let connection = Connection {
      connection_id: cid.clone(),
      a,
      a_public_key: public_key,
      b: target.clone(),
      info_hash: info_hash_of(ticket)?,
      state: ConnectionState::Pending,
      signal_key: None,
      offer: None,
      pending: Vec::new(),
      expires_at: now + self.ttl,
    };
    self.map.lock().insert(cid, connection.clone());
    Ok(connection)
  }

  /// B consents. Only B may, and only while the connection is pending.
  pub fn accept(
    &self,
    connection_id: &str,
    from: &PeerId,
    now: UnixSeconds,
  ) -> Result<Connection, SignalError> {
    let mut map = self.map.lock();
    let c = self.live_mut(&mut map, connection_id, now)?;
    if &c.b != from {
      return Err(SignalError::NotAParticipant);
    }
    if c.state != ConnectionState::Pending {
      return Err(SignalError::NotAccepted);
    }
    c.state = ConnectionState::Accepted;
    Ok(c.clone())
  }

  /// B declines. The connection is over; A gets a distinguishable reason.
  pub fn reject(&self, connection_id: &str, from: &PeerId) -> Result<Connection, SignalError> {
    let mut map = self.map.lock();
    let c = map
      .get(connection_id)
      .ok_or(SignalError::UnknownConnection)?
      .clone();
    if &c.b != from {
      return Err(SignalError::NotAParticipant);
    }
    map.remove(connection_id);
    Ok(c)
  }

  /// A's offer. Refused unless B already accepted, which is what keeps a
  /// rejection free of ICE work.
  pub fn offer(
    &self,
    connection_id: &str,
    from: &PeerId,
    sdp: &str,
    now: UnixSeconds,
  ) -> Result<Connection, SignalError> {
    let mut map = self.map.lock();
    let c = self.live_mut(&mut map, connection_id, now)?;
    if &c.a != from {
      return Err(SignalError::NotAParticipant);
    }
    if c.state == ConnectionState::Pending {
      return Err(SignalError::NotAccepted);
    }
    // a retransmission of the same offer must not hand out a second key
    match &c.offer {
      Some(previous) if previous != sdp => return Err(SignalError::OfferConflict),
      Some(_) => return Ok(c.clone()),
      None => {}
    }
    c.offer = Some(sdp.to_string());
    c.state = ConnectionState::Offered;
    if c.signal_key.is_none() {
      c.signal_key = Some(random_key()?);
    }
    Ok(c.clone())
  }

  /// `answer` / `candidate` / `cancel`: only the two peers, only while it lives.
  pub fn participants(
    &self,
    connection_id: &str,
    from: &PeerId,
    now: UnixSeconds,
  ) -> Result<Connection, SignalError> {
    let mut map = self.map.lock();
    let c = self.live_mut(&mut map, connection_id, now)?;
    if &c.a != from && &c.b != from {
      return Err(SignalError::NotAParticipant);
    }
    Ok(c.clone())
  }

  /// The other end of a connection, given one of them.
  pub fn peer_of(&self, connection_id: &str, from: &PeerId) -> Option<PeerId> {
    let map = self.map.lock();
    let c = map.get(connection_id)?;
    if &c.a == from {
      Some(c.b.clone())
    } else if &c.b == from {
      Some(c.a.clone())
    } else {
      None
    }
  }

  /// Queue signalling for A while it is away, so a resume can replay it.
  pub fn buffer_for_a(&self, connection_id: &str, message: &str) {
    let mut map = self.map.lock();
    if let Some(c) = map.get_mut(connection_id) {
      c.pending.push(message.to_string());
    }
  }

  /// A reconnecting with `signal_key`: the channel replaces the ticket.
  pub fn resume(
    &self,
    connection_id: &str,
    signal_key: &str,
    now: UnixSeconds,
  ) -> Result<(Connection, Vec<String>), SignalError> {
    let mut map = self.map.lock();
    let c = self.live_mut(&mut map, connection_id, now)?;
    let expected = c.signal_key.ok_or(SignalError::BadSignalKey)?;
    let given = B64
      .decode(signal_key)
      .ok()
      .and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok())
      .ok_or(SignalError::BadSignalKey)?;
    if given != expected {
      return Err(SignalError::BadSignalKey);
    }
    let pending = std::mem::take(&mut c.pending);
    Ok((c.clone(), pending))
  }

  pub fn remove(&self, connection_id: &str) -> Option<Connection> {
    self.map.lock().remove(connection_id)
  }

  pub fn get(&self, connection_id: &str) -> Option<Connection> {
    self.map.lock().get(connection_id).cloned()
  }

  pub fn sweep(&self, now: UnixSeconds) -> usize {
    let mut map = self.map.lock();
    let before = map.len();
    map.retain(|_, c| c.expires_at > now);
    let dropped = before - map.len();
    drop(map);

    let mut spent = self.spent.lock();
    spent.retain(|_, until| *until > now);
    dropped
  }

  fn live_mut<'a>(
    &self,
    map: &'a mut HashMap<String, Connection>,
    connection_id: &str,
    now: UnixSeconds,
  ) -> Result<&'a mut Connection, SignalError> {
    let c = map
      .get_mut(connection_id)
      .ok_or(SignalError::UnknownConnection)?;
    if c.expires_at <= now {
      return Err(SignalError::Expired);
    }
    Ok(c)
  }
}

fn random_key() -> Result<[u8; 32], SignalError> {
  let mut out = [0u8; 32];
  getrandom::fill(&mut out).map_err(|_| SignalError::Internal)?;
  Ok(out)
}

#[cfg(test)]
mod tests {
  use super::*;
  use q2q_core::identity::{Ticket, keypair_from_seed, peer_id_of};

  fn ticket(seed: u8, aud: &str, tgt: Option<&PeerId>, cid: &str, now: u64) -> TicketBody {
    let (sk, _) = keypair_from_seed(&[seed; 32]);
    let mut t = Ticket {
      v: 1,
      iss: peer_id_of(&sk),
      public_key: sk.verifying_key().to_bytes(),
      iat: now,
      exp: now + 60,
      aud: Audience::from_wire(aud),
      tgt: tgt.cloned(),
      ih: Some([7u8; 32]),
      cid: Some(cid.into()),
      sig: [0u8; 64],
    };
    t.sign(&sk);
    t.to_body()
  }

  fn peer(seed: u8) -> PeerId {
    let (sk, _) = keypair_from_seed(&[seed; 32]);
    peer_id_of(&sk)
  }

  #[test]
  fn an_offer_before_consent_is_refused() {
    let now = 1_000_000;
    let cs = Connections::new(60);
    let a = peer(1);
    let b = peer(2);
    let t = ticket(1, "punch-1", Some(&b), "cid-1", now);

    cs.open(&t, &b, true, now).unwrap();
    let e = cs.offer("cid-1", &a, "sdp", now).unwrap_err();
    assert_eq!(e, SignalError::NotAccepted);
    assert_eq!(e.code(), "not_accepted");
  }

  #[test]
  fn consent_then_offer_mints_a_signal_key() {
    let now = 1_000_000;
    let cs = Connections::new(60);
    let a = peer(1);
    let b = peer(2);
    let t = ticket(1, "punch-1", Some(&b), "cid-2", now);

    cs.open(&t, &b, true, now).unwrap();
    cs.accept("cid-2", &b, now).unwrap();
    let c = cs.offer("cid-2", &a, "sdp-offer", now).unwrap();
    assert_eq!(c.state, ConnectionState::Offered);
    assert!(c.signal_key.is_some());
  }

  #[test]
  fn the_same_offer_twice_is_idempotent() {
    let now = 1_000_000;
    let cs = Connections::new(60);
    let a = peer(1);
    let b = peer(2);
    let t = ticket(1, "punch-1", Some(&b), "cid-3", now);

    cs.open(&t, &b, true, now).unwrap();
    cs.accept("cid-3", &b, now).unwrap();
    let first = cs.offer("cid-3", &a, "sdp", now).unwrap();
    let second = cs.offer("cid-3", &a, "sdp", now).unwrap();
    assert_eq!(first.signal_key, second.signal_key);
  }

  #[test]
  fn a_different_offer_on_the_same_connection_is_refused() {
    let now = 1_000_000;
    let cs = Connections::new(60);
    let a = peer(1);
    let b = peer(2);
    let t = ticket(1, "punch-1", Some(&b), "cid-4", now);

    cs.open(&t, &b, true, now).unwrap();
    cs.accept("cid-4", &b, now).unwrap();
    cs.offer("cid-4", &a, "sdp-one", now).unwrap();
    assert_eq!(
      cs.offer("cid-4", &a, "sdp-two", now).unwrap_err(),
      SignalError::OfferConflict
    );
  }

  #[test]
  fn a_connect_ticket_is_spent_once() {
    let now = 1_000_000;
    let cs = Connections::new(60);
    let b = peer(2);
    let t = ticket(1, "punch-1", Some(&b), "cid-5", now);

    cs.open(&t, &b, true, now).unwrap();
    // the second attempt must not reveal that the connection exists
    assert_eq!(
      cs.open(&t, &b, true, now).unwrap_err(),
      SignalError::UnknownConnection
    );
  }

  #[test]
  fn a_ticket_for_another_target_is_unauthorized() {
    let now = 1_000_000;
    let cs = Connections::new(60);
    let b = peer(2);
    let other = peer(9);
    let t = ticket(1, "punch-1", Some(&other), "cid-6", now);

    assert_eq!(
      cs.open(&t, &b, true, now).unwrap_err().code(),
      "unauthorized"
    );
  }

  #[test]
  fn an_offline_target_is_reported_distinctly() {
    let now = 1_000_000;
    let cs = Connections::new(60);
    let b = peer(2);
    let t = ticket(1, "punch-1", Some(&b), "cid-7", now);

    assert_eq!(
      cs.open(&t, &b, false, now).unwrap_err(),
      SignalError::PeerOffline
    );
  }

  #[test]
  fn only_the_two_peers_may_relay() {
    let now = 1_000_000;
    let cs = Connections::new(60);
    let a = peer(1);
    let b = peer(2);
    let t = ticket(1, "punch-1", Some(&b), "cid-8", now);

    cs.open(&t, &b, true, now).unwrap();
    cs.accept("cid-8", &b, now).unwrap();
    cs.offer("cid-8", &a, "sdp", now).unwrap();

    assert!(cs.participants("cid-8", &a, now).is_ok());
    assert!(cs.participants("cid-8", &b, now).is_ok());
    assert_eq!(
      cs.participants("cid-8", &peer(3), now).unwrap_err(),
      SignalError::NotAParticipant
    );
  }

  #[test]
  fn a_resume_replays_what_a_missed() {
    let now = 1_000_000;
    let cs = Connections::new(60);
    let a = peer(1);
    let b = peer(2);
    let t = ticket(1, "punch-1", Some(&b), "cid-9", now);

    cs.open(&t, &b, true, now).unwrap();
    cs.accept("cid-9", &b, now).unwrap();
    let c = cs.offer("cid-9", &a, "sdp", now).unwrap();
    let key = B64.encode(c.signal_key.unwrap());

    cs.buffer_for_a("cid-9", "answer-1");
    cs.buffer_for_a("cid-9", "candidate-1");

    let (_, pending) = cs.resume("cid-9", &key, now).unwrap();
    assert_eq!(pending, vec!["answer-1", "candidate-1"]);
    // replayed once, not twice
    let (_, pending) = cs.resume("cid-9", &key, now).unwrap();
    assert!(pending.is_empty());
  }

  #[test]
  fn a_wrong_signal_key_is_refused() {
    let now = 1_000_000;
    let cs = Connections::new(60);
    let a = peer(1);
    let b = peer(2);
    let t = ticket(1, "punch-1", Some(&b), "cid-10", now);

    cs.open(&t, &b, true, now).unwrap();
    cs.accept("cid-10", &b, now).unwrap();
    cs.offer("cid-10", &a, "sdp", now).unwrap();

    let wrong = B64.encode([1u8; 32]);
    assert_eq!(
      cs.resume("cid-10", &wrong, now).unwrap_err(),
      SignalError::BadSignalKey
    );
  }

  #[test]
  fn cancel_drops_the_connection() {
    let now = 1_000_000;
    let cs = Connections::new(60);
    let b = peer(2);
    let t = ticket(1, "punch-1", Some(&b), "cid-11", now);

    cs.open(&t, &b, true, now).unwrap();
    cs.reject("cid-11", &b).unwrap();
    assert!(cs.get("cid-11").is_none());
  }

  #[test]
  fn expired_connections_are_swept() {
    let now = 1_000_000;
    let cs = Connections::new(60);
    let b = peer(2);
    let t = ticket(1, "punch-1", Some(&b), "cid-12", now);

    cs.open(&t, &b, true, now).unwrap();
    assert_eq!(cs.sweep(now + 120), 1);
  }
}
