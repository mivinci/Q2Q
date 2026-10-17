//! Bounded live download attempts; byte notifications are coalesced off the data path.
use q2q_core::identity::PeerId;
use std::{
  sync::{Arc, Mutex},
  time::Duration,
};
use tokio::sync::watch;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadPeerState {
  /// Punch/ICE/identity establishment; peer_id is still a candidate identity.
  Connecting,
  /// Authenticated connection exchanging the initial data-protocol bitfields.
  Handshaking,
  Idle,
  Receiving,
  /// Root verification and durable destination/receipt commit.
  Verifying,
  Finishing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadPeerProgress {
  /// Unique within this download handle, including reconnections to the same peer.
  pub attempt_id: u64,
  pub peer_id: PeerId,
  /// Available only after the connection has authenticated.
  pub connection_id: Option<String>,
  pub state: DownloadPeerState,
  /// Accepted current-block piece payload, including matching duplicates.
  /// Excludes headers, control traffic, ignored old blocks and CDN traffic.
  pub received_bytes: u64,
  /// Blocks verified and durably committed by this attempt; excludes resume data.
  pub verified_bytes: u64,
}

struct State {
  next: u64,
  closed: bool,
  dirty: bool,
  rows: Vec<DownloadPeerProgress>,
}
pub(crate) struct Progress {
  state: Mutex<State>,
  updates: watch::Sender<Vec<DownloadPeerProgress>>,
}
impl Progress {
  pub(crate) fn new() -> Arc<Self> {
    Arc::new(Self {
      state: Mutex::new(State {
        next: 0,
        closed: false,
        dirty: false,
        rows: Vec::new(),
      }),
      updates: watch::channel(Vec::new()).0,
    })
  }
  pub(crate) fn snapshot(&self) -> Vec<DownloadPeerProgress> {
    self
      .state
      .lock()
      .unwrap_or_else(|e| e.into_inner())
      .rows
      .clone()
  }
  pub(crate) fn subscribe(&self) -> watch::Receiver<Vec<DownloadPeerProgress>> {
    let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
    self.flush_locked(&mut state);
    self.updates.subscribe()
  }
  pub(crate) fn connection(self: &Arc<Self>, peer_id: PeerId) -> Arc<Connection> {
    let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
    state.next = state
      .next
      .checked_add(1)
      .expect("download attempt identity exhausted");
    let id = state.next;
    if !state.closed {
      state.rows.push(DownloadPeerProgress {
        attempt_id: id,
        peer_id,
        connection_id: None,
        state: DownloadPeerState::Connecting,
        received_bytes: 0,
        verified_bytes: 0,
      });
      state.dirty = true;
      self.flush_locked(&mut state);
    }
    Arc::new(Connection {
      owner: self.clone(),
      id,
    })
  }
  fn flush_locked(&self, state: &mut State) {
    if state.dirty {
      self.updates.send_replace(state.rows.clone());
      state.dirty = false;
    }
  }
  pub(crate) async fn coalesce(&self) {
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
      tick.tick().await;
      let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
      self.flush_locked(&mut state);
    }
  }
  pub(crate) fn close(&self) {
    let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
    if state.closed {
      return;
    }
    state.closed = true;
    state.rows.clear();
    state.dirty = true;
    self.flush_locked(&mut state);
  }
  fn update(&self, id: u64, immediate: bool, update: impl FnOnce(&mut DownloadPeerProgress)) {
    let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(row) = state.rows.iter_mut().find(|row| row.attempt_id == id) {
      update(row);
      state.dirty = true;
      if immediate {
        self.flush_locked(&mut state);
      }
    }
  }
}
pub(crate) struct Connection {
  owner: Arc<Progress>,
  id: u64,
}
impl Connection {
  pub(crate) fn connected(&self, id: &str, peer_id: &PeerId) {
    self.owner.update(self.id, true, |row| {
      row.peer_id = peer_id.clone();
      row.connection_id = Some(id.into());
      row.state = DownloadPeerState::Handshaking;
    });
  }
  pub(crate) fn state(&self, state: DownloadPeerState) {
    self.owner.update(self.id, true, |row| row.state = state);
  }
  pub(crate) fn received(&self, bytes: usize) {
    self.owner.update(self.id, false, |row| {
      row.received_bytes = row.received_bytes.saturating_add(bytes as u64)
    });
  }
  pub(crate) fn verified(&self, bytes: usize) {
    self.owner.update(self.id, true, |row| {
      row.verified_bytes = row.verified_bytes.saturating_add(bytes as u64)
    });
  }
}
impl Drop for Connection {
  fn drop(&mut self) {
    let mut state = self.owner.state.lock().unwrap_or_else(|e| e.into_inner());
    let before = state.rows.len();
    state.rows.retain(|row| row.attempt_id != self.id);
    if state.rows.len() != before {
      state.dirty = true;
      self.owner.flush_locked(&mut state);
    }
  }
}

#[cfg(test)]
#[path = "../tests/download_progress.rs"]
mod tests;
