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
  fn put_online(&self, record: OnlineRecord);
  fn online(&self, peer_id: &PeerId) -> Option<OnlineRecord>;
  fn touch_sequence(&self, peer_id: &PeerId, seq: u64) -> Result<(), SequenceError>;

  /// Register holdings. `del` is applied before `add`, so the two can be sent
  /// together without a same-hash entry being dropped by ordering.
  fn update_files(
    &self,
    peer_id: &PeerId,
    add: &[InfoHash],
    del: &[InfoHash],
    complete: &[InfoHash],
    ttl: UnixSeconds,
  );

  fn query(
    &self,
    info_hash: &InfoHash,
    limit: usize,
    after: Option<&PeerId>,
    now: UnixSeconds,
  ) -> Vec<Candidate>;

  /// Drop everything that has outlived its TTL. Returns how many went.
  fn sweep(&self, now: UnixSeconds) -> usize;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SequenceError {
  /// Session not established; the client must announce again.
  UnknownSession,
  Replayed(u64),
}

/// In-process state. Fine for one instance; a shared store belongs in a
/// separate crate implementing the same trait.
pub struct MemoryState {
  online: Mutex<HashMap<String, OnlineRecord>>,
  /// info_hash hex -> holdings keyed by peer_id string for stable ordering.
  files: Mutex<BTreeMap<String, BTreeMap<String, Holding>>>,
}

impl MemoryState {
  pub fn new() -> Self {
    MemoryState {
      online: Mutex::new(HashMap::new()),
      files: Mutex::new(BTreeMap::new()),
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
  fn put_online(&self, record: OnlineRecord) {
    let mut online = self.online.lock();
    online.insert(record.peer_id.as_str().to_string(), record);
  }

  fn online(&self, peer_id: &PeerId) -> Option<OnlineRecord> {
    let online = self.online.lock();
    online.get(peer_id.as_str()).cloned()
  }

  fn touch_sequence(&self, peer_id: &PeerId, seq: u64) -> Result<(), SequenceError> {
    let mut online = self.online.lock();
    let record = online
      .get_mut(peer_id.as_str())
      .ok_or(SequenceError::UnknownSession)?;
    if seq <= record.last_seq {
      return Err(SequenceError::Replayed(seq));
    }
    record.last_seq = seq;
    Ok(())
  }

  fn update_files(
    &self,
    peer_id: &PeerId,
    add: &[InfoHash],
    del: &[InfoHash],
    complete: &[InfoHash],
    ttl: UnixSeconds,
  ) {
    let mut files = self.files.lock();
    let expires_at = now() + ttl;

    for h in del {
      if let Some(holders) = files.get_mut(&key(h)) {
        holders.remove(peer_id.as_str());
        if holders.is_empty() {
          files.remove(&key(h));
        }
      }
    }

    for h in add {
      let is_complete = complete.contains(h);
      files.entry(key(h)).or_default().insert(
        peer_id.as_str().to_string(),
        Holding {
          peer_id: peer_id.clone(),
          complete: is_complete,
          expires_at,
        },
      );
    }
  }

  fn query(
    &self,
    info_hash: &InfoHash,
    limit: usize,
    after: Option<&PeerId>,
    now: UnixSeconds,
  ) -> Vec<Candidate> {
    let files = self.files.lock();
    let online = self.online.lock();
    let empty = BTreeMap::new();
    let holders = files.get(&key(info_hash)).unwrap_or(&empty);

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
        let record = online.get(h.peer_id.as_str())?;
        Some(Candidate {
          peer_id: h.peer_id.clone(),
          public_key: record.public_key,
          punch_id: record.punch.punch_id.clone(),
          complete: h.complete,
        })
      })
      .take(limit)
      .collect()
  }

  fn sweep(&self, now: UnixSeconds) -> usize {
    let mut removed = 0;
    let mut online = self.online.lock();
    online.retain(|_, r| {
      let keep = r.expires_at > now;
      if !keep {
        removed += 1;
      }
      keep
    });
    drop(online);

    let mut files = self.files.lock();
    let mut dead = Vec::new();
    for (hash, holders) in files.iter_mut() {
      holders.retain(|_, h| h.expires_at > now);
      if holders.is_empty() {
        dead.push(hash.clone());
      }
    }
    for h in dead {
      files.remove(&h);
    }
    removed
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

  #[test]
  fn query_returns_holders_with_their_punch() {
    let s = MemoryState::new();
    let hash = [1u8; 32];
    s.put_online(online_record(1, 1000));
    s.update_files(&peer(1), &[hash], &[], &[hash], 600);

    let got = s.query(&hash, 10, None, 500);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].peer_id, peer(1));
    assert_eq!(got[0].punch_id, "punch-1");
    assert!(got[0].complete);
  }

  #[test]
  fn del_is_applied_before_add() {
    let s = MemoryState::new();
    let hash = [2u8; 32];
    s.put_online(online_record(2, 1000));
    s.update_files(&peer(2), &[hash], &[], &[], 600);
    assert_eq!(s.query(&hash, 10, None, 500).len(), 1);

    // same request carries both: the entry must survive, not be deleted after
    s.update_files(&peer(2), &[hash], &[hash], &[], 600);
    assert_eq!(s.query(&hash, 10, None, 500).len(), 1);
  }

  #[test]
  fn expired_holders_are_not_returned() {
    let s = MemoryState::new();
    let hash = [3u8; 32];
    s.put_online(online_record(3, 1000));
    s.update_files(&peer(3), &[hash], &[], &[], 100);
    // ttl was 100 from "now", so at now+500 it is gone
    assert_eq!(s.query(&hash, 10, None, now() + 500).len(), 0);
    assert_eq!(s.query(&hash, 10, None, now()).len(), 1);
  }

  #[test]
  fn cursor_pages_through_holders() {
    let s = MemoryState::new();
    let hash = [4u8; 32];
    for n in 1..=5u8 {
      s.put_online(online_record(n, 1000));
      s.update_files(&peer(n), &[hash], &[], &[], 600);
    }
    let page1 = s.query(&hash, 2, None, 500);
    assert_eq!(page1.len(), 2);
    let page2 = s.query(&hash, 2, Some(&page1[1].peer_id), 500);
    assert_eq!(page2.len(), 2);
    // no overlap between pages
    assert_ne!(page1[0].peer_id, page2[0].peer_id);
    assert_ne!(page1[1].peer_id, page2[0].peer_id);
  }

  #[test]
  fn limit_is_respected() {
    let s = MemoryState::new();
    let hash = [5u8; 32];
    for n in 1..=5u8 {
      s.put_online(online_record(n, 1000));
      s.update_files(&peer(n), &[hash], &[], &[], 600);
    }
    assert_eq!(s.query(&hash, 3, None, 500).len(), 3);
  }

  #[test]
  fn complete_flag_only_marks_the_add_subset() {
    let s = MemoryState::new();
    let whole = [6u8; 32];
    let partial = [7u8; 32];
    s.put_online(online_record(6, 1000));
    s.update_files(&peer(6), &[whole, partial], &[], &[whole], 600);

    assert!(s.query(&whole, 10, None, 500)[0].complete);
    assert!(!s.query(&partial, 10, None, 500)[0].complete);
  }

  #[test]
  fn sequence_must_increase() {
    let s = MemoryState::new();
    s.put_online(online_record(8, 1000));
    assert_eq!(s.touch_sequence(&peer(8), 1), Ok(()));
    assert_eq!(s.touch_sequence(&peer(8), 2), Ok(()));
    assert_eq!(
      s.touch_sequence(&peer(8), 2),
      Err(SequenceError::Replayed(2))
    );
    assert_eq!(
      s.touch_sequence(&peer(8), 1),
      Err(SequenceError::Replayed(1))
    );
  }

  #[test]
  fn unknown_session_is_reported() {
    let s = MemoryState::new();
    assert_eq!(
      s.touch_sequence(&peer(9), 1),
      Err(SequenceError::UnknownSession)
    );
  }

  #[test]
  fn sweep_drops_expired_online_records() {
    let s = MemoryState::new();
    s.put_online(online_record(10, 100));
    s.put_online(online_record(11, now() + 10_000));
    let removed = s.sweep(now() + 500);
    assert_eq!(removed, 1);
    assert!(s.online(&peer(10)).is_none());
    assert!(s.online(&peer(11)).is_some());
  }

  #[test]
  fn query_skips_peers_without_an_online_record() {
    let s = MemoryState::new();
    let hash = [12u8; 32];
    // holding registered, but the peer never announced
    s.update_files(&peer(12), &[hash], &[], &[], 600);
    assert_eq!(s.query(&hash, 10, None, 500).len(), 0);
  }
}
