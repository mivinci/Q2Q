//! Who may send which signal, and where it goes next.
//!
//! Two kinds of channel, because they do not live the same life:
//!
//! - **standing** — B's, opened at join. Its presence *is* B's reachability;
//!   when it drops, the binding is gone and connect requests get `peer_offline`.
//! - **ephemeral** — A's, opened with a connection and closed with it.
//!
//! The handler below stays synchronous: it returns the response plus the
//! messages to forward, and the transport does the sending. That keeps the
//! ordering rules testable without a socket.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use parking_lot::Mutex;
use q2q_core::identity::PeerId;
use q2q_punch::message::{Request, Response, SignalResponse};
use q2q_punch::signal::{SignalError, SignalRequest, SignalType, verify_connect};
use quinn::Connection;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Default)]
pub struct Sinks {
  standing: Mutex<HashMap<String, Connection>>,
  ephemeral: Mutex<HashMap<String, Connection>>,
}

impl Sinks {
  pub fn put_standing(&self, peer: &PeerId, conn: Connection) {
    self.standing.lock().insert(peer.as_str().to_string(), conn);
  }

  pub fn drop_standing_if_current(&self, peer: &PeerId, conn: &Connection) {
    let mut standing = self.standing.lock();
    if standing
      .get(peer.as_str())
      .is_some_and(|current| current.stable_id() == conn.stable_id())
    {
      standing.remove(peer.as_str());
    }
  }

  /// Being here is what makes a peer reachable; `connect` asks exactly this.
  pub fn has_standing(&self, peer: &PeerId) -> bool {
    self.standing.lock().contains_key(peer.as_str())
  }

  pub fn put_ephemeral(&self, connection_id: &str, conn: Connection) {
    self
      .ephemeral
      .lock()
      .insert(connection_id.to_string(), conn);
  }

  pub fn drop_ephemeral(&self, connection_id: &str) {
    self.ephemeral.lock().remove(connection_id);
  }

  pub fn ephemeral_is_current(&self, connection_id: &str, conn: &Connection) -> bool {
    self
      .ephemeral
      .lock()
      .get(connection_id)
      .is_some_and(|current| current.stable_id() == conn.stable_id())
  }

  pub fn drop_ephemeral_if_current(&self, connection_id: &str, conn: &Connection) {
    let mut ephemeral = self.ephemeral.lock();
    if ephemeral
      .get(connection_id)
      .is_some_and(|current| current.stable_id() == conn.stable_id())
    {
      ephemeral.remove(connection_id);
    }
  }

  pub fn has_ephemeral(&self, connection_id: &str) -> bool {
    self.ephemeral.lock().contains_key(connection_id)
  }

  /// Push onto B's channel. False means B went away.
  pub async fn to_peer(&self, peer: &PeerId, bytes: &[u8]) -> bool {
    let conn = self.standing.lock().get(peer.as_str()).cloned();
    match conn {
      Some(conn) => push(&conn, bytes).await,
      None => false,
    }
  }

  pub async fn to_initiator(&self, connection_id: &str, bytes: &[u8]) -> bool {
    let conn = self.ephemeral.lock().get(connection_id).cloned();
    match conn {
      Some(conn) => push(&conn, bytes).await,
      None => false,
    }
  }
}

async fn push(conn: &Connection, bytes: &[u8]) -> bool {
  let Ok(mut send) = conn.open_uni().await else {
    return false;
  };
  if send.write_all(bytes).await.is_err() {
    return false;
  }
  send.finish().is_ok()
}

/// Identity bound to the channel the message arrived on, if any.
#[derive(Debug, Clone, Default)]
pub struct Caller {
  pub peer: Option<PeerId>,
}

#[derive(Debug, Clone)]
pub enum Recipient {
  Peer(PeerId),
  Initiator(String),
}

/// A message the transport owes to someone else.
#[derive(Debug, Clone)]
pub struct Forward {
  pub to: Recipient,
  pub message: String,
}

pub struct Outcome {
  pub response: Response,
  pub forwards: Vec<Forward>,
  /// Set when the caller just joined: it now owns a standing channel.
  pub joined: Option<PeerId>,
  /// Set when the caller opened or resumed a connection: its channel is the
  /// ephemeral one for that connection id.
  pub ephemeral: Option<String>,
}

impl Outcome {
  fn ok(req: &Request, resp: SignalResponse, forwards: Vec<Forward>) -> Outcome {
    Outcome {
      response: Response::ok(&req.id, &resp),
      forwards,
      joined: None,
      ephemeral: None,
    }
  }

  fn err(req: &Request, e: SignalError) -> Outcome {
    Outcome {
      response: Response::err(&req.id, e.code(), Some(e.to_string())),
      forwards: Vec::new(),
      joined: None,
      ephemeral: None,
    }
  }
}

pub fn handle_signal(
  state: &crate::server::State,
  req: &Request,
  sig: &SignalRequest,
  caller: &Caller,
  now: u64,
) -> Outcome {
  match sig.kind {
    SignalType::Connect => connect(state, req, sig, now),
    SignalType::Accept => accept(state, req, sig, caller, now),
    SignalType::Reject => reject(state, req, sig, caller, now),
    SignalType::Offer => offer(state, req, sig, caller, now),
    SignalType::Answer | SignalType::Candidate => relay(state, req, sig, caller, now),
    SignalType::Cancel => cancel(state, req, sig, caller, now),
  }
}

fn connect(state: &crate::server::State, req: &Request, sig: &SignalRequest, now: u64) -> Outcome {
  let Some(token) = sig.connect_token.as_deref() else {
    return Outcome::err(req, SignalError::MissingTicket);
  };
  let ticket = match verify_connect(token, &state.punch_id, now) {
    Ok(t) => t,
    Err(e) => return Outcome::err(req, e),
  };
  let Some(target) = ticket.tgt.as_ref().map(PeerId::unchecked) else {
    return Outcome::err(req, SignalError::NotTheTarget);
  };

  // reachability is decided by the standing channel, not by any table
  let online = state.sinks.has_standing(&target);
  let connection = match state.connections.open(&ticket, &target, online, now) {
    Ok(c) => c,
    Err(e) => return Outcome::err(req, e),
  };

  let message = match serde_json::to_string(sig) {
    Ok(m) => m,
    Err(_) => return Outcome::err(req, SignalError::MissingTicket),
  };
  tracing::info!(
    connection = %connection.connection_id,
    from = %connection.a.as_str(),
    to = %target.as_str(),
    "connect"
  );

  let mut outcome = Outcome::ok(
    req,
    SignalResponse {
      ok: true,
      signal_key: None,
      pending: Vec::new(),
    },
    vec![Forward {
      to: Recipient::Peer(target),
      message,
    }],
  );
  outcome.ephemeral = Some(connection.connection_id);
  outcome
}

fn accept(
  state: &crate::server::State,
  req: &Request,
  sig: &SignalRequest,
  caller: &Caller,
  now: u64,
) -> Outcome {
  let Some(from) = caller.peer.as_ref() else {
    return Outcome::err(req, SignalError::NotAParticipant);
  };
  if let Err(e) = state.connections.accept(&sig.connection_id, from, now) {
    return Outcome::err(req, e);
  }
  forward_to_other(state, req, sig, from, "accept")
}

fn reject(
  state: &crate::server::State,
  req: &Request,
  sig: &SignalRequest,
  caller: &Caller,
  _now: u64,
) -> Outcome {
  let Some(from) = caller.peer.as_ref() else {
    return Outcome::err(req, SignalError::NotAParticipant);
  };
  if let Err(e) = state.connections.reject(&sig.connection_id, from) {
    return Outcome::err(req, e);
  }
  forward_to_other(state, req, sig, from, "reject")
}

fn offer(
  state: &crate::server::State,
  req: &Request,
  sig: &SignalRequest,
  // A has no binding: the ticket is the credential, not the channel
  _caller: &Caller,
  now: u64,
) -> Outcome {
  // a reconnect carries the key instead of a fresh offer
  if let Some(key) = sig.signal_key.as_deref() {
    let mut outcome = match state.connections.resume(&sig.connection_id, key, now) {
      Ok((_, pending)) => Outcome::ok(
        req,
        SignalResponse {
          ok: true,
          signal_key: None,
          pending,
        },
        Vec::new(),
      ),
      Err(e) => return Outcome::err(req, e),
    };
    outcome.ephemeral = Some(sig.connection_id.clone());
    return outcome;
  }

  let Some(token) = sig.connect_token.as_deref() else {
    return Outcome::err(req, SignalError::MissingTicket);
  };
  let ticket = match verify_connect(token, &state.punch_id, now) {
    Ok(t) => t,
    Err(e) => return Outcome::err(req, e),
  };
  let a = PeerId::unchecked(&ticket.iss);
  let sdp = sig.sdp.clone().unwrap_or_default();

  let connection = match state.connections.offer(&sig.connection_id, &a, &sdp, now) {
    Ok(c) => c,
    Err(e) => return Outcome::err(req, e),
  };

  let message = match serde_json::to_string(sig) {
    Ok(m) => m,
    Err(_) => return Outcome::err(req, SignalError::MissingTicket),
  };
  let target = connection.b.clone();
  let mut outcome = Outcome::ok(
    req,
    SignalResponse {
      ok: true,
      signal_key: connection.signal_key.map(|k| B64.encode(k)),
      pending: Vec::new(),
    },
    vec![Forward {
      to: Recipient::Peer(target),
      message,
    }],
  );
  outcome.ephemeral = Some(sig.connection_id.clone());
  outcome
}

fn relay(
  state: &crate::server::State,
  req: &Request,
  sig: &SignalRequest,
  caller: &Caller,
  now: u64,
) -> Outcome {
  let Some(from) = caller.peer.as_ref() else {
    return Outcome::err(req, SignalError::NotAParticipant);
  };
  if let Err(e) = state
    .connections
    .participants(&sig.connection_id, from, now)
  {
    return Outcome::err(req, e);
  }
  let _ = caller;
  forward_to_other(state, req, sig, from, sig.kind.name())
}

fn cancel(
  state: &crate::server::State,
  req: &Request,
  sig: &SignalRequest,
  caller: &Caller,
  now: u64,
) -> Outcome {
  let Some(from) = caller.peer.as_ref() else {
    return Outcome::err(req, SignalError::NotAParticipant);
  };
  if let Err(e) = state
    .connections
    .participants(&sig.connection_id, from, now)
  {
    return Outcome::err(req, e);
  }
  let outcome = forward_to_other(state, req, sig, from, "cancel");
  state.connections.remove(&sig.connection_id);
  state.sinks.drop_ephemeral(&sig.connection_id);
  outcome
}

/// Send to whoever is on the other side of the connection.
fn forward_to_other(
  state: &crate::server::State,
  req: &Request,
  sig: &SignalRequest,
  from: &PeerId,
  what: &str,
) -> Outcome {
  let message = match serde_json::to_string(sig) {
    Ok(m) => m,
    Err(_) => return Outcome::err(req, SignalError::MissingTicket),
  };

  let other = state.connections.peer_of(&sig.connection_id, from);
  let Some(other) = other else {
    return Outcome::err(req, SignalError::NotAParticipant);
  };

  // A's channel is ephemeral and may be between reconnects; hold the message
  // rather than dropping it, and replay on resume
  if &other == from {
    return Outcome::err(req, SignalError::NotAParticipant);
  }
  let to = if state
    .connections
    .get(&sig.connection_id)
    .is_some_and(|c| c.a == other)
  {
    Recipient::Initiator(sig.connection_id.clone())
  } else {
    Recipient::Peer(other)
  };

  if let Recipient::Initiator(cid) = &to
    && !state.sinks.has_ephemeral(cid)
  {
    state.connections.buffer_for_a(cid, &message);
    tracing::debug!(connection = %cid, what, "buffered for the initiator");
    return Outcome::ok(
      req,
      SignalResponse {
        ok: true,
        signal_key: None,
        pending: Vec::new(),
      },
      Vec::new(),
    );
  }

  Outcome::ok(
    req,
    SignalResponse {
      ok: true,
      signal_key: None,
      pending: Vec::new(),
    },
    vec![Forward { to, message }],
  )
}

/// Run whatever the handler asked for.
pub async fn deliver(sinks: &Arc<Sinks>, forwards: Vec<Forward>) {
  for f in forwards {
    let delivered = match &f.to {
      Recipient::Peer(peer) => sinks.to_peer(peer, f.message.as_bytes()).await,
      Recipient::Initiator(cid) => sinks.to_initiator(cid, f.message.as_bytes()).await,
    };
    if !delivered {
      tracing::debug!(?f.to, "forward dropped: channel is gone");
    }
  }
}
