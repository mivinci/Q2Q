//! Who may send which signal, and where it goes next.
//!
//! Two kinds of channel, because they do not live the same life:
//!
//! - **standing** — B's, opened at join and authorised by its current binding.
//!   A missing channel, expired lease or exit makes the peer unreachable.
//! - **ephemeral** — A's, opened with a connection and closed with it.
//!
//! The handler below stays synchronous: it returns the response plus the
//! messages to forward, and the transport does the sending. That keeps the
//! ordering rules testable without a socket.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use parking_lot::Mutex;
use q2q_primitives::identity::PeerId;
use q2q_punch::message::{Request, Response, SignalResponse};
use q2q_punch::signal::{SignalError, SignalRequest, SignalType, verify_connect};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use tokio::sync::{mpsc, oneshot};

const DELIVERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

static NEXT_SINK_ID: AtomicU64 = AtomicU64::new(1);

/// A transport-neutral route for signals pushed to a live Punch channel.
/// The transport adapter owns the receiver and performs the actual QUIC stream
/// or WebSocket frame write.
#[derive(Clone)]
pub struct ChannelSink {
  id: u64,
  sender: mpsc::Sender<PendingPush>,
}

pub struct PendingPush {
  pub signal: SignalRequest,
  pub(crate) delivered: oneshot::Sender<bool>,
}

impl ChannelSink {
  pub fn channel(capacity: usize) -> (ChannelSink, mpsc::Receiver<PendingPush>) {
    let (sender, receiver) = mpsc::channel(capacity.max(1));
    (
      ChannelSink {
        id: NEXT_SINK_ID.fetch_add(1, Ordering::Relaxed),
        sender,
      },
      receiver,
    )
  }

  fn is_same(&self, other: &ChannelSink) -> bool {
    self.id == other.id
  }

  async fn push(&self, signal: &SignalRequest) -> bool {
    let (delivered, result) = oneshot::channel();
    tokio::time::timeout(DELIVERY_TIMEOUT, async {
      if self
        .sender
        .send(PendingPush {
          signal: signal.clone(),
          delivered,
        })
        .await
        .is_err()
      {
        return false;
      }
      result.await.unwrap_or(false)
    })
    .await
    .unwrap_or(false)
  }
}

struct StandingSink {
  binding_id: String,
  sink: ChannelSink,
}

#[derive(Default)]
pub struct Sinks {
  standing: Mutex<HashMap<String, StandingSink>>,
  serial: Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>,
  ephemeral: Mutex<HashMap<String, ChannelSink>>,
}

impl Sinks {
  pub fn put_standing(
    &self,
    binding: &q2q_punch::binding::Binding,
    sink: ChannelSink,
    bindings: &q2q_punch::binding::Bindings,
  ) -> bool {
    let mut standing = self.standing.lock();
    // Keep the route lock while checking the binding: an older join must not
    // overwrite a route installed by a newer join for the same identity.
    if bindings.find(&binding.binding_id).is_none() {
      return false;
    }
    standing.insert(
      binding.peer_id.as_str().to_string(),
      StandingSink {
        binding_id: binding.binding_id.clone(),
        sink,
      },
    );
    true
  }

  pub fn drop_standing_if_current(&self, peer: &PeerId, sink: &ChannelSink) {
    let mut standing = self.standing.lock();
    if standing
      .get(peer.as_str())
      .is_some_and(|current| current.sink.is_same(sink))
    {
      standing.remove(peer.as_str());
    }
  }

  pub fn standing_is_current(&self, peer: &PeerId, binding_id: &str, sink: &ChannelSink) -> bool {
    self
      .standing
      .lock()
      .get(peer.as_str())
      .is_some_and(|current| current.binding_id == binding_id && current.sink.is_same(sink))
  }

  #[cfg(test)]
  pub fn has_standing(&self, peer: &PeerId) -> bool {
    self.standing.lock().contains_key(peer.as_str())
  }

  pub fn has_live_standing(
    &self,
    peer: &PeerId,
    bindings: &q2q_punch::binding::Bindings,
    now: u64,
  ) -> bool {
    let mut standing = self.standing.lock();
    let live = standing.get(peer.as_str()).is_some_and(|route| {
      bindings
        .find(&route.binding_id)
        .is_some_and(|binding| binding.expires_at > now)
    });
    if !live {
      standing.remove(peer.as_str());
    }
    live
  }

  /// Serialise one connection's signalling through route changes and delivery.
  /// This closes the gap between buffering an undelivered message and resume.
  pub fn connection_lock(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
    let mut serial = self.serial.lock();
    if let Some(lock) = serial.get(id).and_then(Weak::upgrade) {
      return lock;
    }
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    serial.insert(id.to_string(), Arc::downgrade(&lock));
    lock
  }

  pub fn sweep(
    &self,
    bindings: &q2q_punch::binding::Bindings,
    connections: &q2q_punch::signal::Connections,
    now: u64,
  ) {
    self.standing.lock().retain(|_, route| {
      bindings
        .find(&route.binding_id)
        .is_some_and(|binding| binding.expires_at > now)
    });
    self.ephemeral.lock().retain(|id, _| {
      connections
        .get(id)
        .is_some_and(|connection| connection.expires_at > now)
    });
    self.serial.lock().retain(|_, lock| lock.strong_count() > 0);
  }

  pub fn put_ephemeral(&self, connection_id: &str, sink: ChannelSink) {
    self
      .ephemeral
      .lock()
      .insert(connection_id.to_string(), sink);
  }

  pub fn drop_ephemeral(&self, connection_id: &str) {
    self.ephemeral.lock().remove(connection_id);
  }

  pub fn ephemeral_is_current(&self, connection_id: &str, sink: &ChannelSink) -> bool {
    self
      .ephemeral
      .lock()
      .get(connection_id)
      .is_some_and(|current| current.is_same(sink))
  }

  pub fn drop_ephemeral_if_current(&self, connection_id: &str, sink: &ChannelSink) {
    let mut ephemeral = self.ephemeral.lock();
    if ephemeral
      .get(connection_id)
      .is_some_and(|current| current.is_same(sink))
    {
      ephemeral.remove(connection_id);
    }
  }

  pub fn has_ephemeral(&self, connection_id: &str) -> bool {
    self.ephemeral.lock().contains_key(connection_id)
  }

  /// Push onto B's channel. False means B went away.
  pub async fn to_peer(&self, peer: &PeerId, signal: &SignalRequest) -> bool {
    let conn = self
      .standing
      .lock()
      .get(peer.as_str())
      .map(|route| route.sink.clone());
    match conn {
      Some(conn) => conn.push(signal).await,
      None => false,
    }
  }

  pub async fn to_initiator(&self, connection_id: &str, signal: &SignalRequest) -> bool {
    let conn = self.ephemeral.lock().get(connection_id).cloned();
    match conn {
      Some(conn) => conn.push(signal).await,
      None => false,
    }
  }
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
  pub signal: SignalRequest,
}

pub struct Outcome {
  pub response: Response,
  pub forwards: Vec<Forward>,
  /// Set when the caller just joined: it now owns a standing channel.
  pub joined: Option<q2q_punch::binding::Binding>,
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

  if ticket.cid.as_deref() != Some(sig.connection_id.as_str()) {
    return Outcome::err(req, SignalError::NotTheTarget);
  }
  let online = state.sinks.has_live_standing(&target, &state.bindings, now);
  let connection = match state.connections.open(&ticket, &target, online, now) {
    Ok(c) => c,
    Err(e) => return Outcome::err(req, e),
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
      signal: sig.clone(),
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
  forward_to_other(state, req, sig, from, "accept", now)
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
  // Reject removes the connection, so route the terminal signal directly.
  Outcome::ok(
    req,
    SignalResponse {
      ok: true,
      signal_key: None,
      pending: Vec::new(),
    },
    vec![Forward {
      to: Recipient::Initiator(sig.connection_id.clone()),
      signal: sig.clone(),
    }],
  )
}

fn offer(
  state: &crate::server::State,
  req: &Request,
  sig: &SignalRequest,
  caller: &Caller,
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
  let Some(existing) = state.connections.get(&sig.connection_id) else {
    return Outcome::err(req, SignalError::UnknownConnection);
  };
  let hash = match q2q_punch::signal::info_hash_of(&ticket) {
    Ok(hash) => hash,
    Err(error) => return Outcome::err(req, error),
  };
  if ticket.cid.as_deref() != Some(sig.connection_id.as_str())
    || ticket.tgt.as_deref() != Some(existing.b.as_str())
    || hash != existing.info_hash
    || ticket.public_key != B64.encode(existing.a_public_key)
  {
    return Outcome::err(req, SignalError::NotTheTarget);
  }
  let a = PeerId::unchecked(&ticket.iss);
  // The target also sees the forwarded ticket. Only the initiator's channel
  // may turn it into an offer and receive the private resume credential.
  if caller.peer.as_ref() != Some(&a) {
    return Outcome::err(req, SignalError::NotAParticipant);
  }
  let sdp = sig.sdp.clone().unwrap_or_default();

  let connection = match state.connections.offer(&sig.connection_id, &a, &sdp, now) {
    Ok(c) => c,
    Err(e) => return Outcome::err(req, e),
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
      signal: sig.clone(),
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
  forward_to_other(state, req, sig, from, sig.kind.name(), now)
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
  let outcome = forward_to_other(state, req, sig, from, "cancel", now);
  state.connections.remove(&sig.connection_id);
  outcome
}

/// Send to whoever is on the other side of the connection.
fn forward_to_other(
  state: &crate::server::State,
  req: &Request,
  sig: &SignalRequest,
  from: &PeerId,
  what: &str,
  now: u64,
) -> Outcome {
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
    if matches!(sig.kind, SignalType::Cancel | SignalType::Reject) {
      return Outcome::err(req, SignalError::DeliveryFailed);
    }
    let message = match serde_json::to_string(sig) {
      Ok(message) => message,
      Err(_) => return Outcome::err(req, SignalError::MissingTicket),
    };
    if !state.connections.buffer_for_a(cid, &message, now) {
      return Outcome::err(req, SignalError::DeliveryFailed);
    }
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
    vec![Forward {
      to,
      signal: sig.clone(),
    }],
  )
}

/// Run whatever the handler asked for.
pub async fn deliver(
  state: &crate::server::State,
  forwards: Vec<Forward>,
) -> Result<(), &'static str> {
  for f in forwards {
    let delivered = match &f.to {
      Recipient::Peer(peer) => {
        state
          .sinks
          .has_live_standing(peer, &state.bindings, q2q_punch::binding::now())
          && state.sinks.to_peer(peer, &f.signal).await
      }
      Recipient::Initiator(cid) => state.sinks.to_initiator(cid, &f.signal).await,
    };
    let terminal = matches!(f.signal.kind, SignalType::Cancel | SignalType::Reject);
    if terminal {
      state.sinks.drop_ephemeral(&f.signal.connection_id);
    }
    if !delivered {
      if !terminal && let Recipient::Initiator(cid) = &f.to {
        let message = serde_json::to_string(&f.signal).map_err(|_| "signal_delivery_failed")?;
        if state
          .connections
          .buffer_for_a(cid, &message, q2q_punch::binding::now())
        {
          continue;
        }
      }
      return Err("signal_delivery_failed");
    }
  }
  Ok(())
}
