//! Exclusive block leases and notifications, independent of transport workers.
use crate::download_policy::{DownloadPolicy, SourceKind};
use crate::error::TransferError;
use crate::progress::{self, TransferProgress};
use crate::task_scope::{TaskGuard, TaskScope};
use q2q_core::seed::Seed;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;

struct State {
  pending: VecDeque<usize>,
  done: Vec<bool>,
  remaining: usize,
  leased: Vec<bool>,
  policy: DownloadPolicy,
}

pub(crate) struct WorkQueue {
  state: Mutex<State>,
  changed: watch::Sender<u64>,
  focus_changed: watch::Sender<u64>,
  pub(crate) progress: watch::Sender<TransferProgress>,
  length: u64,
  block_size: u32,
  pub(crate) scope: TaskScope,
}

impl WorkQueue {
  pub(crate) fn new(
    seed: &Seed,
    bits: &[u8],
    progress: watch::Sender<TransferProgress>,
  ) -> Arc<Self> {
    Self::scoped(seed, bits, progress, TaskScope::new())
  }

  pub(crate) fn scoped(
    seed: &Seed,
    bits: &[u8],
    progress: watch::Sender<TransferProgress>,
    scope: TaskScope,
  ) -> Arc<Self> {
    let done: Vec<_> = (0..seed.info.leaf_count())
      .map(|i| {
        bits
          .get(i / 8)
          .is_some_and(|byte| byte & (1 << (7 - i % 8)) != 0)
      })
      .collect();
    let pending: VecDeque<_> = (0..done.len()).filter(|i| !done[*i]).collect();
    Arc::new(Self {
      state: Mutex::new(State {
        remaining: pending.len(),
        leased: vec![false; done.len()],
        policy: DownloadPolicy::default(),
        pending,
        done,
      }),
      changed: watch::channel(0).0,
      focus_changed: watch::channel(0).0,
      progress,
      length: seed.info.length,
      block_size: seed.info.block_size,
      scope,
    })
  }

  pub(crate) fn demand(
    seed: &Seed,
    bits: &[u8],
    progress: watch::Sender<TransferProgress>,
  ) -> Arc<Self> {
    let queue = Self::new(seed, bits, progress);
    {
      let mut state = queue.state.lock().unwrap_or_else(|e| e.into_inner());
      let count = state.done.len();
      state.pending.clear();
      state.policy = DownloadPolicy::window(count);
    }
    queue
  }

  fn notify(&self) {
    self.changed.send_modify(|v| *v = v.wrapping_add(1));
  }

  pub(crate) fn subscribe_focus(&self) -> watch::Receiver<u64> {
    self.focus_changed.subscribe()
  }

  pub(crate) fn focus(&self, index: usize, ahead: usize, wait: Duration, urgent: bool) {
    let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
    let end = index
      .saturating_add(ahead)
      .saturating_add(1)
      .min(state.done.len());
    state.policy.focus(index, end, wait, urgent);
    self.focus_changed.send_modify(|v| *v = v.wrapping_add(1));
    state.pending = (index..end)
      .filter(|i| !state.done[*i] && !state.leased[*i])
      .collect();
    drop(state);
    self.notify();
  }

  pub(crate) fn peer_status(&self, exhausted: bool, error: Option<TransferError>) {
    let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
    state.policy.peer_status(exhausted, error);
    drop(state);
    self.notify();
  }

  pub(crate) fn cdn_status(&self, result: Result<(), TransferError>) {
    let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
    state.policy.cdn_status(result);
    drop(state);
    self.notify();
  }

  pub(crate) fn fail(&self, message: TransferError) {
    self
      .state
      .lock()
      .unwrap_or_else(|e| e.into_inner())
      .policy
      .fail(message);
    progress::finish(&self.progress, progress::TransferState::Failed);
    self.notify();
  }

  pub(crate) fn block_failed(&self, index: usize, error: TransferError) {
    self
      .state
      .lock()
      .unwrap_or_else(|e| e.into_inner())
      .policy
      .block_failed(index, error);
    self.notify();
  }

  pub(crate) async fn verified(&self, index: usize) -> std::io::Result<()> {
    let mut changed = self.changed.subscribe();
    loop {
      {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(error) = state.policy.fatal() {
          return Err(std::io::Error::other(error.clone()));
        }
        if state.done[index] {
          return Ok(());
        }
        if let Some(error) = state.policy.read_error(index) {
          return Err(std::io::Error::other(error.clone()));
        }
      }
      changed.changed().await.expect("queue sender");
    }
  }

  pub(crate) async fn cdn_needed(&self) {
    let mut changed = self.changed.subscribe();
    loop {
      let deadline = {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.remaining == 0 || (state.policy.peers_exhausted() && !state.pending.is_empty()) {
          return;
        }
        state.policy.deadline(&state.done)
      };
      if let Some(deadline) = deadline {
        tokio::select! { _ = tokio::time::sleep_until(deadline) => return, _ = changed.changed() => {} }
      } else {
        changed.changed().await.expect("queue sender");
      }
    }
  }

  pub(crate) fn subscribe(&self) -> watch::Receiver<u64> {
    self.changed.subscribe()
  }
  pub(crate) fn peer_deadline(&self, index: usize) -> Option<tokio::time::Instant> {
    self
      .state
      .lock()
      .unwrap_or_else(|e| e.into_inner())
      .policy
      .peer_deadline(index)
  }
  pub(crate) async fn claim_cdn(self: &Arc<Self>) -> Option<BlockLease> {
    self.claim_from(SourceKind::Cdn).await
  }

  pub(crate) fn complete(&self) -> bool {
    self
      .state
      .lock()
      .unwrap_or_else(|e| e.into_inner())
      .remaining
      == 0
  }

  pub(crate) fn bitfield(&self) -> Vec<u8> {
    let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
    let mut bits = vec![0; state.done.len().div_ceil(8)];
    for (i, done) in state.done.iter().enumerate() {
      if *done {
        bits[i / 8] |= 1 << (7 - i % 8);
      }
    }
    bits
  }

  pub(crate) async fn claim(self: &Arc<Self>) -> Option<BlockLease> {
    self.claim_from(SourceKind::Peer).await
  }
  async fn claim_from(self: &Arc<Self>, source: SourceKind) -> Option<BlockLease> {
    let mut changed = self.changed.subscribe();
    loop {
      let deadline = {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.remaining == 0 || state.policy.fatal().is_some() {
          return None;
        }
        let position = state
          .pending
          .iter()
          .position(|index| state.policy.eligible(*index, source));
        if let Some(index) = position.and_then(|position| state.pending.remove(position)) {
          state.leased[index] = true;
          return Some(BlockLease {
            index,
            queue: self.clone(),
            committed: false,
          });
        }
        match source {
          SourceKind::Cdn => state
            .policy
            .deadline(&state.done)
            .filter(|deadline| *deadline > tokio::time::Instant::now()),
          SourceKind::Peer => None,
        }
      };
      if let Some(deadline) = deadline {
        tokio::select! { _ = tokio::time::sleep_until(deadline) => {}, _ = changed.changed() => {} }
      } else {
        changed.changed().await.expect("queue sender");
      }
    }
  }

  pub(crate) async fn finished(&self) {
    let mut changed = self.changed.subscribe();
    while {
      let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
      state.remaining != 0 && state.policy.fatal().is_none()
    } {
      changed.changed().await.expect("queue retains its sender");
    }
  }
}

pub(crate) struct BlockLease {
  pub(crate) index: usize,
  queue: Arc<WorkQueue>,
  committed: bool,
}
impl BlockLease {
  pub(crate) fn disk_task(&self) -> TaskGuard {
    self.queue.scope.enter()
  }

  pub(crate) fn commit(mut self) {
    {
      let mut state = self.queue.state.lock().unwrap_or_else(|e| e.into_inner());
      state.leased[self.index] = false;
      state.done[self.index] = true;
      state.remaining -= 1;
    }
    self.committed = true;
    self.queue.progress.send_modify(|p| {
      p.verified_blocks += 1;
      p.verified_bytes += progress::block_len(self.queue.length, self.queue.block_size, self.index);
    });
    self
      .queue
      .changed
      .send_modify(|version| *version = version.wrapping_add(1));
  }
}
impl Drop for BlockLease {
  fn drop(&mut self) {
    if !self.committed {
      let mut state = self.queue.state.lock().unwrap_or_else(|e| e.into_inner());
      state.leased[self.index] = false;
      if state.policy.contains(self.index) {
        state.pending.push_front(self.index);
      }
      drop(state);
      self
        .queue
        .changed
        .send_modify(|version| *version = version.wrapping_add(1));
    }
  }
}
