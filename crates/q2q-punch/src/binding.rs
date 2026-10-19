//! Bindings: a peer's standing relationship with its Punch Server.
//!
//! A binding is what makes a peer reachable for signalling. It is established
//! once with a ticket, then kept alive with a symmetric MAC rather than a
//! signature — a heartbeat every few seconds does not want an Ed25519 verify
//! each time.

use parking_lot::Mutex;
use q2q_primitives::identity::PeerId;
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
#[derive(Default)]
struct BindingTable {
  by_peer: HashMap<String, Binding>,
  by_id: HashMap<String, String>,
}

pub struct Bindings {
  table: Mutex<BindingTable>,
  spent: Mutex<HashMap<String, UnixSeconds>>,
  ttl: UnixSeconds,
}

impl Bindings {
  pub fn new(ttl: UnixSeconds) -> Self {
    Self {
      table: Mutex::new(BindingTable::default()),
      spent: Mutex::new(HashMap::new()),
      ttl,
    }
  }

  pub fn spend(&self, token_id: &str, now: UnixSeconds, ticket_exp: UnixSeconds) -> bool {
    let mut spent = self.spent.lock();
    if spent.get(token_id).is_some_and(|until| *until > now) {
      return false;
    }
    spent.insert(
      token_id.to_string(),
      now + ticket_exp.saturating_sub(now) * 2 + self.ttl,
    );
    true
  }

  pub fn insert(&self, binding: Binding) {
    let mut table = self.table.lock();
    if let Some(old) = table
      .by_peer
      .insert(binding.peer_id.as_str().to_string(), binding.clone())
    {
      table.by_id.remove(&old.binding_id);
    }
    table
      .by_id
      .insert(binding.binding_id, binding.peer_id.as_str().to_string());
  }

  pub fn get(&self, peer_id: &PeerId) -> Option<Binding> {
    self.table.lock().by_peer.get(peer_id.as_str()).cloned()
  }

  pub fn find(&self, binding_id: &str) -> Option<Binding> {
    let table = self.table.lock();
    table.by_peer.get(table.by_id.get(binding_id)?).cloned()
  }

  pub fn remove(&self, peer_id: &PeerId) -> Option<Binding> {
    let mut table = self.table.lock();
    let old = table.by_peer.remove(peer_id.as_str())?;
    table.by_id.remove(&old.binding_id);
    Some(old)
  }

  /// Transport cleanup must never remove a replacement binding for the same peer.
  pub fn remove_if_current(&self, binding_id: &str) -> Option<Binding> {
    let mut table = self.table.lock();
    let peer = table.by_id.remove(binding_id)?;
    table.by_peer.remove(&peer)
  }

  pub fn live(&self, peer_id: &PeerId, now: UnixSeconds) -> Result<Binding, BindingError> {
    let b = self.get(peer_id).ok_or(BindingError::Unknown)?;
    if b.expires_at <= now {
      return Err(BindingError::Expired);
    }
    Ok(b)
  }

  /// Authenticate against the current key and advance the lease in one critical section.
  pub fn heartbeat_authenticated(
    &self,
    binding_id: &str,
    seq: u64,
    generation: Generation,
    now: UnixSeconds,
    verify: impl FnOnce(&Binding) -> bool,
  ) -> Result<Binding, BindingError> {
    let mut table = self.table.lock();
    let peer = table
      .by_id
      .get(binding_id)
      .ok_or(BindingError::Unknown)?
      .clone();
    let b = table.by_peer.get_mut(&peer).ok_or(BindingError::Unknown)?;
    validate(b, seq, generation, now, verify)?;
    b.last_seq = seq;
    b.generation = generation;
    b.expires_at = now + self.ttl;
    Ok(b.clone())
  }

  pub fn exit_authenticated(
    &self,
    binding_id: &str,
    seq: u64,
    generation: Generation,
    now: UnixSeconds,
    verify: impl FnOnce(&Binding) -> bool,
  ) -> Result<Binding, BindingError> {
    let mut table = self.table.lock();
    let peer = table
      .by_id
      .get(binding_id)
      .ok_or(BindingError::Unknown)?
      .clone();
    let b = table.by_peer.get(&peer).ok_or(BindingError::Unknown)?;
    validate(b, seq, generation, now, verify)?;
    let removed = table.by_peer.remove(&peer).ok_or(BindingError::Unknown)?;
    table.by_id.remove(binding_id);
    Ok(removed)
  }

  /// Refresh a previously authenticated peer. Wire handlers use the binding-id
  /// variant above so a concurrent rejoin cannot change the key being checked.
  pub fn heartbeat(
    &self,
    peer_id: &PeerId,
    seq: u64,
    generation: Generation,
    mac_ok: bool,
    now: UnixSeconds,
  ) -> Result<Binding, BindingError> {
    let mut table = self.table.lock();
    let b = table
      .by_peer
      .get_mut(peer_id.as_str())
      .ok_or(BindingError::Unknown)?;
    validate(b, seq, generation, now, |_| mac_ok)?;
    b.last_seq = seq;
    b.generation = generation;
    b.expires_at = now + self.ttl;
    Ok(b.clone())
  }

  pub fn sweep(&self, now: UnixSeconds) -> usize {
    let mut table = self.table.lock();
    let before = table.by_peer.len();
    table.by_peer.retain(|_, b| b.expires_at > now);
    let BindingTable { by_peer, by_id } = &mut *table;
    by_id.retain(|_, peer| by_peer.contains_key(peer));
    let dropped = before - by_peer.len();
    drop(table);
    self.spent.lock().retain(|_, until| *until > now);
    dropped
  }

  pub fn len(&self) -> usize {
    self.table.lock().by_peer.len()
  }
  pub fn is_empty(&self) -> bool {
    self.len() == 0
  }
}

fn validate(
  binding: &Binding,
  seq: u64,
  generation: Generation,
  now: UnixSeconds,
  verify: impl FnOnce(&Binding) -> bool,
) -> Result<(), BindingError> {
  if !verify(binding) {
    return Err(BindingError::BadMac);
  }
  if binding.expires_at <= now {
    return Err(BindingError::Expired);
  }
  if generation < binding.generation {
    return Err(BindingError::StaleGeneration(generation));
  }
  if seq <= binding.last_seq {
    return Err(BindingError::StaleSequence(seq));
  }
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;
  use q2q_primitives::identity::peer_id_from_public_key;

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
  #[test]
  fn concurrent_replays_advance_the_sequence_only_once() {
    let bindings = std::sync::Arc::new(store());
    bindings.insert(binding(1, 1000));
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(16));
    let accepted = std::thread::scope(|scope| {
      let handles: Vec<_> = (0..16)
        .map(|_| {
          let bindings = bindings.clone();
          let barrier = barrier.clone();
          scope.spawn(move || {
            barrier.wait();
            bindings
              .heartbeat_authenticated("b1", 1, 0, 900, |_| true)
              .is_ok()
          })
        })
        .collect();
      handles
        .into_iter()
        .map(|handle| usize::from(handle.join().unwrap()))
        .sum::<usize>()
    });
    assert_eq!(accepted, 1);
  }

  #[test]
  fn old_lease_operations_cannot_replace_or_remove_a_new_join() {
    let bindings = store();
    bindings.insert(binding(1, 1000));
    let mut replacement = binding(1, 2000);
    replacement.binding_id = "replacement".into();
    bindings.insert(replacement.clone());
    assert_eq!(
      bindings.heartbeat_authenticated("b1", 99, 0, 900, |_| true),
      Err(BindingError::Unknown)
    );
    assert_eq!(
      bindings.exit_authenticated("b1", 99, 0, 900, |_| true),
      Err(BindingError::Unknown)
    );
    assert!(bindings.remove_if_current("b1").is_none());
    assert_eq!(bindings.find("replacement"), Some(replacement));
  }
}
