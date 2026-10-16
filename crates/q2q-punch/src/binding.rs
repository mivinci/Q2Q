//! Bindings: a peer's standing relationship with its Punch Server.
//!
//! A binding is what makes a peer reachable for signalling. It is established
//! once with a ticket, then kept alive with a symmetric MAC rather than a
//! signature — a heartbeat every few seconds does not want an Ed25519 verify
//! each time.

use parking_lot::Mutex;
use q2q_core::identity::PeerId;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

pub type UnixSeconds = u64;
/// Bumped on ICE restart, so a heartbeat minted before a network switch cannot
/// be replayed afterwards to keep a stale NAT mapping alive.
pub type Generation = u64;

pub fn now() -> UnixSeconds {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|d| d.as_secs())
    .unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
  pub binding_id: String,
  pub peer_id: PeerId,
  pub key: [u8; 32],
  pub expires_at: UnixSeconds,
  /// Highest sequence number accepted so far.
  pub last_seq: u64,
  pub generation: Generation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingError {
  Unknown,
  Expired,
  /// Sequence went backwards, or repeated: either a replay or reordering.
  StaleSequence(u64),
  /// Generation is behind the one the server has seen.
  StaleGeneration(Generation),
  BadMac,
}

impl std::fmt::Display for BindingError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      BindingError::Unknown => write!(f, "no such binding"),
      BindingError::Expired => write!(f, "binding expired"),
      BindingError::StaleSequence(seq) => write!(f, "sequence {seq} already seen"),
      BindingError::StaleGeneration(g) => write!(f, "generation {g} is behind"),
      BindingError::BadMac => write!(f, "bad mac"),
    }
  }
}

impl std::error::Error for BindingError {}

impl BindingError {
  pub fn code(&self) -> &'static str {
    match self {
      BindingError::Unknown => "unknown_binding",
      BindingError::Expired => "binding_expired",
      BindingError::StaleSequence(_) => "stale_sequence",
      BindingError::StaleGeneration(_) => "stale_generation",
      BindingError::BadMac => "bad_mac",
    }
  }
}

/// Track's state is behind a trait because several instances share it; a
/// binding belongs to exactly one Punch Server, so this one is concrete.
pub struct Bindings {
  /// peer_id → binding. Joining again replaces the previous one.
  by_peer: Mutex<HashMap<String, Binding>>,
  /// binding_id → peer_id. Heartbeats and exits only carry the binding id.
  by_id: Mutex<HashMap<String, String>>,
  /// One-shot ticket ids, kept for twice the ticket lifetime.
  spent: Mutex<HashMap<String, UnixSeconds>>,
  ttl: UnixSeconds,
}

impl Bindings {
  pub fn new(ttl: UnixSeconds) -> Self {
    Bindings {
      by_peer: Mutex::new(HashMap::new()),
      by_id: Mutex::new(HashMap::new()),
      spent: Mutex::new(HashMap::new()),
      ttl,
    }
  }

  /// Mark a ticket id as used. Returns false if it was already spent, which is
  /// what stops a captured join from being replayed.
  pub fn spend(&self, token_id: &str, now: UnixSeconds, ticket_exp: UnixSeconds) -> bool {
    let mut spent = self.spent.lock();
    if spent.get(token_id).is_some_and(|until| *until > now) {
      return false;
    }
    // keep it for twice the ticket's remaining life
    spent.insert(
      token_id.to_string(),
      now + (ticket_exp.saturating_sub(now)) * 2 + self.ttl,
    );
    true
  }

  pub fn insert(&self, binding: Binding) {
    let mut map = self.by_peer.lock();
    // joining again replaces the old binding, so drop the stale id first
    if let Some(old) = map.get(binding.peer_id.as_str())
      && old.binding_id != binding.binding_id
    {
      self.by_id.lock().remove(&old.binding_id);
    }
    self.by_id.lock().insert(
      binding.binding_id.clone(),
      binding.peer_id.as_str().to_string(),
    );
    map.insert(binding.peer_id.as_str().to_string(), binding);
  }

  pub fn get(&self, peer_id: &PeerId) -> Option<Binding> {
    let map = self.by_peer.lock();
    map.get(peer_id.as_str()).cloned()
  }

  pub fn find(&self, binding_id: &str) -> Option<Binding> {
    let peer = self.by_id.lock().get(binding_id).cloned()?;
    self.get(&PeerId::unchecked(peer))
  }

  pub fn remove(&self, peer_id: &PeerId) -> Option<Binding> {
    let mut map = self.by_peer.lock();
    let old = map.remove(peer_id.as_str())?;
    self.by_id.lock().remove(&old.binding_id);
    Some(old)
  }

  /// Look up a binding that is still alive.
  pub fn live(&self, peer_id: &PeerId, now: UnixSeconds) -> Result<Binding, BindingError> {
    let b = self.get(peer_id).ok_or(BindingError::Unknown)?;
    if b.expires_at <= now {
      return Err(BindingError::Expired);
    }
    Ok(b)
  }

  /// Accept a heartbeat: check mac, generation and sequence, then extend.
  ///
  /// `mac_ok` is passed in rather than computed here so this stays about state;
  /// the wire layer owns the MAC construction.
  pub fn heartbeat(
    &self,
    peer_id: &PeerId,
    seq: u64,
    generation: Generation,
    mac_ok: bool,
    now: UnixSeconds,
  ) -> Result<Binding, BindingError> {
    if !mac_ok {
      return Err(BindingError::BadMac);
    }
    let b = self.live(peer_id, now)?;
    if generation < b.generation {
      return Err(BindingError::StaleGeneration(generation));
    }
    if seq <= b.last_seq {
      return Err(BindingError::StaleSequence(seq));
    }

    let updated = Binding {
      expires_at: now + self.ttl,
      last_seq: seq,
      generation: generation.max(b.generation),
      ..b
    };
    self.insert(updated.clone());
    Ok(updated)
  }

  /// Drop expired bindings and spent-token records. Call it on a timer.
  pub fn sweep(&self, now: UnixSeconds) -> usize {
    let mut map = self.by_peer.lock();
    let before = map.len();
    map.retain(|_, b| b.expires_at > now);
    let dropped = before - map.len();

    let mut ids = self.by_id.lock();
    ids.retain(|_, peer| map.contains_key(peer));
    drop(ids);
    drop(map);

    let mut spent = self.spent.lock();
    spent.retain(|_, until| *until > now);
    dropped
  }

  pub fn len(&self) -> usize {
    self.by_peer.lock().len()
  }

  pub fn is_empty(&self) -> bool {
    self.len() == 0
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use q2q_core::identity::peer_id_from_public_key;

  fn peer(n: u8) -> PeerId {
    peer_id_from_public_key(&[n; 32])
  }

  fn binding(n: u8, expires_at: UnixSeconds) -> Binding {
    Binding {
      binding_id: format!("b{n}"),
      peer_id: peer(n),
      key: [n ^ 0x55; 32],
      expires_at,
      last_seq: 0,
      generation: 0,
    }
  }

  fn store() -> Bindings {
    Bindings::new(600)
  }

  #[test]
  fn joining_twice_replaces_the_binding() {
    let b = store();
    b.insert(binding(1, 1000));
    let mut second = binding(1, 2000);
    second.binding_id = "b1-second".into();
    b.insert(second);

    let got = b.get(&peer(1)).unwrap();
    assert_eq!(got.binding_id, "b1-second");
    assert_eq!(b.len(), 1);
  }

  #[test]
  fn a_spent_token_cannot_be_reused() {
    let b = store();
    assert!(b.spend("cid-1", 100, 200));
    assert!(!b.spend("cid-1", 100, 200), "replay must be refused");
  }

  #[test]
  fn a_spent_token_eventually_expires() {
    let b = store();
    b.spend("cid-1", 100, 110);
    // well past the retained window
    assert!(b.spend("cid-1", 100_000, 100_100));
  }

  #[test]
  fn heartbeat_extends_and_advances() {
    let b = store();
    b.insert(binding(2, 1000));
    let after = b.heartbeat(&peer(2), 5, 0, true, 900).unwrap();
    assert_eq!(after.last_seq, 5);
    assert!(after.expires_at > 1000);
  }

  #[test]
  fn replayed_sequence_is_refused() {
    let b = store();
    b.insert(binding(3, 1000));
    b.heartbeat(&peer(3), 5, 0, true, 900).unwrap();
    assert_eq!(
      b.heartbeat(&peer(3), 5, 0, true, 900),
      Err(BindingError::StaleSequence(5))
    );
    assert_eq!(
      b.heartbeat(&peer(3), 4, 0, true, 900),
      Err(BindingError::StaleSequence(4))
    );
  }

  #[test]
  fn generation_may_only_move_forward() {
    let b = store();
    b.insert(binding(4, 1000));
    b.heartbeat(&peer(4), 1, 7, true, 900).unwrap();
    // an old generation cannot come back after a network switch
    assert_eq!(
      b.heartbeat(&peer(4), 2, 3, true, 900),
      Err(BindingError::StaleGeneration(3))
    );
    assert!(b.heartbeat(&peer(4), 3, 8, true, 900).is_ok());
  }

  #[test]
  fn bad_mac_is_refused_before_anything_else() {
    let b = store();
    b.insert(binding(5, 1000));
    assert_eq!(
      b.heartbeat(&peer(5), 1, 0, false, 900),
      Err(BindingError::BadMac)
    );
  }

  #[test]
  fn expired_binding_is_reported() {
    let b = store();
    b.insert(binding(6, 1000));
    assert_eq!(b.live(&peer(6), 1001), Err(BindingError::Expired));
    assert!(b.live(&peer(6), 999).is_ok());
  }

  #[test]
  fn exit_removes_the_binding() {
    let b = store();
    b.insert(binding(7, 1000));
    assert!(b.remove(&peer(7)).is_some());
    assert!(b.get(&peer(7)).is_none());
  }

  #[test]
  fn sweep_drops_expired_bindings() {
    let b = store();
    b.insert(binding(8, 100));
    b.insert(binding(9, now() + 10_000));
    assert_eq!(b.sweep(now() + 500), 1);
    assert_eq!(b.len(), 1);
  }
}
