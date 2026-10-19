//! Publication snapshots, bounded live connection records and coalesced updates.
use q2q_primitives::identity::PeerId;
use std::sync::{
  Arc, Mutex,
  atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::sync::watch;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationState {
  Publishing,
  Closing,
  Closed,
  Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadConnectionState {
  Handshaking,
  /// Authenticated, with no requested block awaiting acknowledgement.
  Idle,
  /// Reading, waiting for budget, sending or awaiting BlockDone.
  Sending,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadConnectionProgress {
  pub connection_id: String,
  /// Ticket identity during handshake; authenticated identity afterwards.
  pub peer_id: PeerId,
  pub state: UploadConnectionState,
  /// Encoded piece bytes accepted by DataChannel, including retransmits.
  pub sent_bytes: u64,
  /// Subset of sent_bytes sent in response to PieceNack.
  pub retransmitted_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadProgress {
  pub state: PublicationState,
  pub handshaking_connections: usize,
  /// Connections in Sending state, including budget and acknowledgement waits.
  pub active_connections: usize,
  pub idle_connections: usize,
  /// Encoded piece bytes accepted by DataChannel, including retransmits.
  pub sent_bytes: u64,
  /// Subset of sent_bytes sent in response to PieceNack.
  pub retransmitted_bytes: u64,
  /// Normally ended sessions, including sessions serving only part of a file.
  pub completed_connections: u64,
  pub failed_connections: u64,
  /// Workers cancelled locally, including publication shutdown.
  pub cancelled_connections: u64,
  /// Live admitted connections only; finished records are removed.
  pub connections: Vec<UploadConnectionProgress>,
}
struct State {
  snapshot: UploadProgress,
  dirty: bool,
}
pub(crate) struct Progress {
  state: Mutex<State>,
  updates: watch::Sender<UploadProgress>,
}
impl Progress {
  pub(crate) fn new() -> Arc<Self> {
    let snapshot = UploadProgress {
      state: PublicationState::Publishing,
      handshaking_connections: 0,
      active_connections: 0,
      idle_connections: 0,
      sent_bytes: 0,
      retransmitted_bytes: 0,
      completed_connections: 0,
      failed_connections: 0,
      cancelled_connections: 0,
      connections: Vec::new(),
    };
    Arc::new(Self {
      updates: watch::channel(snapshot.clone()).0,
      state: Mutex::new(State {
        snapshot,
        dirty: false,
      }),
    })
  }
  pub(crate) fn snapshot(&self) -> UploadProgress {
    self
      .state
      .lock()
      .unwrap_or_else(|e| e.into_inner())
      .snapshot
      .clone()
  }
  pub(crate) fn subscribe(&self) -> watch::Receiver<UploadProgress> {
    let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
    Self::publish(&mut state, &self.updates);
    self.updates.subscribe()
  }
  fn publish(state: &mut State, updates: &watch::Sender<UploadProgress>) {
    if state.dirty {
      updates.send_replace(state.snapshot.clone());
      state.dirty = false;
    }
  }
  fn update(&self, immediate: bool, update: impl FnOnce(&mut UploadProgress) -> bool) {
    let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
    let changed = update(&mut state.snapshot);
    state.dirty |= changed;
    if immediate
      || matches!(
        state.snapshot.state,
        PublicationState::Closed | PublicationState::Failed
      )
    {
      Self::publish(&mut state, &self.updates);
    }
  }
  pub(crate) fn flush(&self) {
    let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
    Self::publish(&mut state, &self.updates);
  }
  pub(crate) async fn coalesce(&self) {
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
      tick.tick().await;
      self.flush();
    }
  }
  pub(crate) fn closing(&self) {
    self.update(true, |p| {
      if p.state == PublicationState::Publishing {
        p.state = PublicationState::Closing;
        true
      } else {
        false
      }
    });
  }
  pub(crate) fn finish(&self, failed: bool) {
    self.update(true, |p| {
      if matches!(
        p.state,
        PublicationState::Publishing | PublicationState::Closing
      ) {
        p.state = if failed {
          PublicationState::Failed
        } else {
          PublicationState::Closed
        };
        true
      } else {
        false
      }
    });
  }
  pub(crate) fn run_guard(self: &Arc<Self>) -> RunGuard {
    RunGuard(self.clone())
  }
  pub(crate) fn connection(
    self: &Arc<Self>,
    connection_id: String,
    peer_id: PeerId,
  ) -> Arc<Connection> {
    self.update(false, |p| {
      p.connections.push(UploadConnectionProgress {
        connection_id: connection_id.clone(),
        peer_id,
        state: UploadConnectionState::Handshaking,
        sent_bytes: 0,
        retransmitted_bytes: 0,
      });
      recount(p);
      true
    });
    Arc::new(Connection {
      progress: self.clone(),
      id: connection_id,
      finished: AtomicBool::new(false),
    })
  }
}
fn recount(p: &mut UploadProgress) {
  p.handshaking_connections = p
    .connections
    .iter()
    .filter(|c| c.state == UploadConnectionState::Handshaking)
    .count();
  p.active_connections = p
    .connections
    .iter()
    .filter(|c| c.state == UploadConnectionState::Sending)
    .count();
  p.idle_connections = p
    .connections
    .iter()
    .filter(|c| c.state == UploadConnectionState::Idle)
    .count();
}
pub(crate) struct RunGuard(Arc<Progress>);
impl Drop for RunGuard {
  fn drop(&mut self) {
    self.0.finish(true);
  }
}
#[derive(Clone, Copy)]
pub(crate) enum Outcome {
  Completed,
  Failed,
  Cancelled,
}
pub(crate) struct Connection {
  progress: Arc<Progress>,
  id: String,
  finished: AtomicBool,
}
impl Connection {
  pub(crate) fn state(&self, state: UploadConnectionState) {
    if self.finished.load(Ordering::Acquire) {
      return;
    }
    self.progress.update(false, |p| {
      if let Some(connection) = p
        .connections
        .iter_mut()
        .find(|c| c.connection_id == self.id)
      {
        if connection.state == state {
          return false;
        }
        connection.state = state;
        recount(p);
        true
      } else {
        false
      }
    });
  }
  pub(crate) fn sent(&self, bytes: usize, retransmit: bool) {
    if self.finished.load(Ordering::Acquire) {
      return;
    }
    self.progress.update(false, |p| {
      if let Some(connection) = p
        .connections
        .iter_mut()
        .find(|c| c.connection_id == self.id)
      {
        connection.sent_bytes = connection.sent_bytes.saturating_add(bytes as u64);
        p.sent_bytes = p.sent_bytes.saturating_add(bytes as u64);
        if retransmit {
          connection.retransmitted_bytes =
            connection.retransmitted_bytes.saturating_add(bytes as u64);
          p.retransmitted_bytes = p.retransmitted_bytes.saturating_add(bytes as u64);
        }
        true
      } else {
        false
      }
    });
  }
  pub(crate) fn finish(&self, outcome: Outcome) {
    // A completed guard can outlive its row. Never settle a later session
    // reusing this connection ID when the old guard is finally dropped.
    if self.finished.swap(true, Ordering::AcqRel) {
      return;
    }
    self.progress.update(false, |p| {
      if let Some(index) = p
        .connections
        .iter()
        .position(|c| c.connection_id == self.id)
      {
        p.connections.remove(index);
        let counter = match outcome {
          Outcome::Completed => &mut p.completed_connections,
          Outcome::Failed => &mut p.failed_connections,
          Outcome::Cancelled => &mut p.cancelled_connections,
        };
        *counter = counter.saturating_add(1);
        recount(p);
        true
      } else {
        false
      }
    });
  }
}
impl Drop for Connection {
  fn drop(&mut self) {
    self.finish(Outcome::Cancelled);
  }
}

#[cfg(test)]
#[path = "../tests/upload_progress.rs"]
mod tests;
