//! Track's state: who is online, and who holds what.
//!
//! The trait exists so the service logic does not care where state lives. A
//! single instance can use `MemoryState`; several instances behind a load
//! balancer need something shared, which is why `session_key` is part of the
//! trait rather than process-local memory — if announce lands on one instance
//! and query on another, the lookup has to succeed.

use parking_lot::Mutex;
use q2q_core::identity::PeerId;
use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::time::{SystemTime, UNIX_EPOCH};

pub type InfoHash = [u8; 32];
pub type UnixSeconds = u64;

pub fn now() -> UnixSeconds {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|d| d.as_secs())
    .unwrap_or(0)
}

/// One peer's online record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnlineRecord {
  pub peer_id: PeerId,
  /// Public key from the announce ticket, cached for whoever queries later.
  pub public_key: [u8; 32],
  pub session_key: [u8; 32],
  /// Highest sequence number seen on this session, for replay protection.
  pub last_seq: u64,
  pub punch: PunchAssignment,
  pub expires_at: UnixSeconds,
}

/// The Punch Server a peer was assigned to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PunchAssignment {
  pub punch_id: String,
  pub url: String,
  pub public_key: Vec<u8>,
}

/// One candidate returned from a query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
  pub peer_id: PeerId,
  pub public_key: [u8; 32],
  /// Which Punch Server the peer is bound to, by id. The address and key are
  /// resolved from the live list at query time — a snapshot would keep handing
  /// out a stale address after the instance moves.
  pub punch_id: String,
  pub complete: bool,
}

/// An entry in the file index.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Holding {
  peer_id: PeerId,
  complete: bool,
  expires_at: UnixSeconds,
}

pub trait TrackState: Send + Sync {
  /// Establish a new session and update holdings as one operation. `del` is
  /// applied before `add`, including when a hash appears in both lists.
  fn announce(
    &self,
    record: OnlineRecord,
    add: &[InfoHash],
    del: &[InfoHash],
    complete: &[InfoHash],
    ttl: UnixSeconds,
  ) -> impl Future<Output = Result<(), StateError>> + Send;

  fn online(
    &self,
    peer_id: &PeerId,
    now: UnixSeconds,
  ) -> impl Future<Output = Result<Option<OnlineRecord>, StateError>> + Send;

  /// Compare the session observed by the caller, advance its sequence and
  /// optionally renew its expiry under one lock / storage transaction.
  fn advance_session(
    &self,
    peer_id: &PeerId,
    session_key: &[u8; 32],
    seq: u64,
    renew_until: Option<UnixSeconds>,
    now: UnixSeconds,
  ) -> impl Future<Output = Result<(), StateError>> + Send;

  fn query(
    &self,
    info_hash: &InfoHash,
    limit: usize,
    after: Option<&PeerId>,
    now: UnixSeconds,
  ) -> impl Future<Output = Result<Vec<Candidate>, StateError>> + Send;

  /// Drop everything that has outlived its TTL. Returns how many went.
  fn sweep(&self, now: UnixSeconds) -> impl Future<Output = Result<usize, StateError>> + Send;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateError {
  /// Session not established; the client must announce again.
  UnknownSession,
  Replayed(u64),
  /// A shared backend could not complete the operation.
  Unavailable(String),
}

/// In-process state for a single Track instance. The optional Redis backend
/// implements the same trait when multiple instances share state.
pub struct MemoryState {
  data: Mutex<MemoryData>,
}

struct MemoryData {
  online: HashMap<String, OnlineRecord>,
  /// info_hash hex -> holdings keyed by peer_id string for stable ordering.
  files: BTreeMap<String, BTreeMap<String, Holding>>,
}

impl MemoryState {
  pub fn new() -> Self {
    MemoryState {
      data: Mutex::new(MemoryData {
        online: HashMap::new(),
        files: BTreeMap::new(),
      }),
    }
  }
}

impl Default for MemoryState {
  fn default() -> Self {
    Self::new()
  }
}

fn key(h: &InfoHash) -> String {
  h.iter().map(|b| format!("{b:02x}")).collect()
}

impl TrackState for MemoryState {
  async fn announce(
    &self,
    record: OnlineRecord,
    add: &[InfoHash],
    del: &[InfoHash],
    complete: &[InfoHash],
    _ttl: UnixSeconds,
  ) -> Result<(), StateError> {
    let mut data = self.data.lock();
    let peer_id = record.peer_id.clone();
    let expires_at = record.expires_at;
    data.online.insert(peer_id.as_str().to_string(), record);

    for h in del {
      if let Some(holders) = data.files.get_mut(&key(h)) {
        holders.remove(peer_id.as_str());
        if holders.is_empty() {
          data.files.remove(&key(h));
        }
      }
    }

    for h in add {
      let is_complete = complete.contains(h);
      data.files.entry(key(h)).or_default().insert(
        peer_id.as_str().to_string(),
        Holding {
          peer_id: peer_id.clone(),
          complete: is_complete,
          expires_at,
        },
      );
    }
    Ok(())
  }

  async fn online(
    &self,
    peer_id: &PeerId,
    now: UnixSeconds,
  ) -> Result<Option<OnlineRecord>, StateError> {
    Ok(
      self
        .data
        .lock()
        .online
        .get(peer_id.as_str())
        .filter(|r| r.expires_at > now)
        .cloned(),
    )
  }

  async fn advance_session(
    &self,
    peer_id: &PeerId,
    session_key: &[u8; 32],
    seq: u64,
    renew_until: Option<UnixSeconds>,
    now: UnixSeconds,
  ) -> Result<(), StateError> {
    let mut data = self.data.lock();
    let record = data
      .online
      .get_mut(peer_id.as_str())
      .ok_or(StateError::UnknownSession)?;
    if record.expires_at <= now || &record.session_key != session_key {
      return Err(StateError::UnknownSession);
    }
    if seq <= record.last_seq {
      return Err(StateError::Replayed(seq));
    }
    record.last_seq = seq;
    if let Some(expires_at) = renew_until {
      record.expires_at = expires_at;
    }
    Ok(())
  }

  async fn query(
    &self,
    info_hash: &InfoHash,
    limit: usize,
    after: Option<&PeerId>,
    now: UnixSeconds,
  ) -> Result<Vec<Candidate>, StateError> {
    let data = self.data.lock();
    let empty = BTreeMap::new();
    let holders = data.files.get(&key(info_hash)).unwrap_or(&empty);

    Ok(
      holders
        .values()
        .filter(|h| h.expires_at > now)
        .filter(|h| {
          // cursor: only peers strictly after the last one handed out
          match after {
            Some(p) => h.peer_id.as_str() > p.as_str(),
            None => true,
          }
        })
        .filter_map(|h| {
          let record = data
            .online
            .get(h.peer_id.as_str())
            .filter(|r| r.expires_at > now)?;
          Some(Candidate {
            peer_id: h.peer_id.clone(),
            public_key: record.public_key,
            punch_id: record.punch.punch_id.clone(),
            complete: h.complete,
          })
        })
        .take(limit)
        .collect(),
    )
  }

  async fn sweep(&self, now: UnixSeconds) -> Result<usize, StateError> {
    let mut removed = 0;
    let mut data = self.data.lock();
    data.online.retain(|_, r| {
      let keep = r.expires_at > now;
      if !keep {
        removed += 1;
      }
      keep
    });

    let mut dead = Vec::new();
    for (hash, holders) in data.files.iter_mut() {
      holders.retain(|_, h| h.expires_at > now);
      if holders.is_empty() {
        dead.push(hash.clone());
      }
    }
    for h in dead {
      data.files.remove(&h);
    }
    Ok(removed)
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use q2q_core::identity::peer_id_from_public_key;

  fn peer(n: u8) -> PeerId {
    peer_id_from_public_key(&[n; 32])
  }

  fn punch(n: u8) -> PunchAssignment {
    PunchAssignment {
      punch_id: format!("punch-{n}"),
      url: format!("https://punch-{n}.example.com"),
      public_key: vec![n; 32],
    }
  }

  fn online_record(n: u8, expires_at: UnixSeconds) -> OnlineRecord {
    OnlineRecord {
      peer_id: peer(n),
      public_key: [n; 32],
      session_key: [n ^ 0xff; 32],
      last_seq: 0,
      punch: punch(n),
      expires_at,
    }
  }

  #[tokio::test]
  async fn query_returns_holders_with_their_punch() {
    let s = MemoryState::new();
    let hash = [1u8; 32];
    s.announce(online_record(1, 1000), &[hash], &[], &[hash], 600)
      .await
      .unwrap();

    let got = s.query(&hash, 10, None, 500).await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].peer_id, peer(1));
    assert_eq!(got[0].punch_id, "punch-1");
    assert!(got[0].complete);
  }

  #[tokio::test]
  async fn del_is_applied_before_add() {
    let s = MemoryState::new();
    let hash = [2u8; 32];
    s.announce(online_record(2, 1000), &[hash], &[], &[], 600)
      .await
      .unwrap();
    assert_eq!(s.query(&hash, 10, None, 500).await.unwrap().len(), 1);

    // same request carries both: the entry must survive, not be deleted after
    s.announce(online_record(2, 1000), &[hash], &[hash], &[], 600)
      .await
      .unwrap();
    assert_eq!(s.query(&hash, 10, None, 500).await.unwrap().len(), 1);
  }

  #[tokio::test]
  async fn expired_holders_are_not_returned() {
    let s = MemoryState::new();
    let hash = [3u8; 32];
    s.announce(online_record(3, now() + 100), &[hash], &[], &[], 100)
      .await
      .unwrap();
    // ttl was 100 from "now", so at now+500 it is gone
    assert_eq!(
      s.query(&hash, 10, None, now() + 500).await.unwrap().len(),
      0
    );
    assert_eq!(s.query(&hash, 10, None, now()).await.unwrap().len(), 1);
  }

  #[tokio::test]
  async fn cursor_pages_through_holders() {
    let s = MemoryState::new();
    let hash = [4u8; 32];
    for n in 1..=5u8 {
      s.announce(online_record(n, 1000), &[hash], &[], &[], 600)
        .await
        .unwrap();
    }
    let page1 = s.query(&hash, 2, None, 500).await.unwrap();
    assert_eq!(page1.len(), 2);
    let page2 = s
      .query(&hash, 2, Some(&page1[1].peer_id), 500)
      .await
      .unwrap();
    assert_eq!(page2.len(), 2);
    // no overlap between pages
    assert_ne!(page1[0].peer_id, page2[0].peer_id);
    assert_ne!(page1[1].peer_id, page2[0].peer_id);
  }

  #[tokio::test]
  async fn limit_is_respected() {
    let s = MemoryState::new();
    let hash = [5u8; 32];
    for n in 1..=5u8 {
      s.announce(online_record(n, 1000), &[hash], &[], &[], 600)
        .await
        .unwrap();
    }
    assert_eq!(s.query(&hash, 3, None, 500).await.unwrap().len(), 3);
  }

  #[tokio::test]
  async fn complete_flag_only_marks_the_add_subset() {
    let s = MemoryState::new();
    let whole = [6u8; 32];
    let partial = [7u8; 32];
    s.announce(
      online_record(6, 1000),
      &[whole, partial],
      &[],
      &[whole],
      600,
    )
    .await
    .unwrap();

    assert!(s.query(&whole, 10, None, 500).await.unwrap()[0].complete);
    assert!(!s.query(&partial, 10, None, 500).await.unwrap()[0].complete);
  }

  #[tokio::test]
  async fn sequence_must_increase() {
    let s = MemoryState::new();
    s.announce(online_record(8, 1000), &[], &[], &[], 600)
      .await
      .unwrap();
    let key = [8 ^ 0xff; 32];
    assert_eq!(
      s.advance_session(&peer(8), &key, 1, None, 500).await,
      Ok(())
    );
    assert_eq!(
      s.advance_session(&peer(8), &key, 2, None, 500).await,
      Ok(())
    );
    assert_eq!(
      s.advance_session(&peer(8), &key, 2, None, 500).await,
      Err(StateError::Replayed(2))
    );
    assert_eq!(
      s.advance_session(&peer(8), &key, 1, None, 500).await,
      Err(StateError::Replayed(1))
    );
  }

  #[tokio::test]
  async fn unknown_session_is_reported() {
    let s = MemoryState::new();
    assert_eq!(
      s.advance_session(&peer(9), &[0; 32], 1, None, 500).await,
      Err(StateError::UnknownSession)
    );
  }

  #[tokio::test]
  async fn sweep_drops_expired_online_records() {
    let s = MemoryState::new();
    s.announce(online_record(10, 100), &[], &[], &[], 600)
      .await
      .unwrap();
    s.announce(online_record(11, now() + 10_000), &[], &[], &[], 600)
      .await
      .unwrap();
    let removed = s.sweep(now() + 500).await.unwrap();
    assert_eq!(removed, 1);
    assert!(s.online(&peer(10), now() + 500).await.unwrap().is_none());
    assert!(s.online(&peer(11), now() + 500).await.unwrap().is_some());
  }

  #[tokio::test]
  async fn query_skips_peers_without_an_online_record() {
    let s = MemoryState::new();
    let hash = [12u8; 32];
    // holding registered, but the peer never announced
    s.data.lock().files.entry(key(&hash)).or_default().insert(
      peer(12).as_str().to_string(),
      Holding {
        peer_id: peer(12),
        complete: false,
        expires_at: 1000,
      },
    );
    assert_eq!(s.query(&hash, 10, None, 500).await.unwrap().len(), 0);
  }
}
