//! Bounded peer workers sharing exclusive block leases.

use crate::download_store::DownloadStore;
use crate::peer::PeerSession;
use crate::progress::{self, TransferProgress};
use crate::transfer::{TransferError, destination_complete};
use q2q_core::seed::Seed;
use std::collections::VecDeque;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinSet;

/// Tracks network workers and their non-abortable disk jobs across source phases.
#[derive(Clone)]
pub(crate) struct TaskScope(watch::Sender<usize>);
impl TaskScope {
  pub(crate) fn new() -> Self {
    Self(watch::channel(0).0)
  }
  pub(crate) fn enter(&self) -> TaskGuard {
    self.0.send_modify(|count| *count += 1);
    TaskGuard {
      scope: self.clone(),
    }
  }
  pub(crate) async fn idle(&self) {
    let mut count = self.0.subscribe();
    loop {
      if *count.borrow_and_update() == 0 {
        return;
      }
      count.changed().await.expect("scope retains its sender");
    }
  }
}
pub(crate) struct TaskGuard {
  scope: TaskScope,
}
impl Drop for TaskGuard {
  fn drop(&mut self) {
    self.scope.0.send_modify(|count| *count -= 1);
  }
}

struct State {
  pending: VecDeque<usize>,
  done: Vec<bool>,
  remaining: usize,
  leased: Vec<bool>,
  demand: Option<Demand>,
}

struct Demand {
  start: usize,
  end: usize,
  urgent: Option<(usize, tokio::time::Instant)>,
  cdn_ready: bool,
  peers_exhausted: bool,
  errors: Vec<Option<String>>,
  source_error: Option<String>,
  fatal: Option<String>,
}

pub(crate) struct WorkQueue {
  state: Mutex<State>,
  changed: watch::Sender<u64>,
  focus_changed: watch::Sender<u64>,
  pub(crate) progress: watch::Sender<TransferProgress>,
  length: u64,
  block_size: u32,
  scope: TaskScope,
}

impl WorkQueue {
  pub(crate) fn new(
    seed: &Seed,
    bits: &[u8],
    progress: watch::Sender<TransferProgress>,
  ) -> Arc<Self> {
    Self::scoped(seed, bits, progress, TaskScope::new())
  }

  fn scoped(
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
        demand: None,
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
      state.demand = Some(Demand {
        start: 0,
        end: 0,
        urgent: None,
        cdn_ready: false,
        peers_exhausted: false,
        errors: vec![None; count],
        source_error: None,
        fatal: None,
      });
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
    if let Some(d) = &mut state.demand {
      d.start = index;
      d.end = end;
      d.urgent = urgent.then(|| (index, tokio::time::Instant::now() + wait));
      for error in d.errors.iter_mut().take(end).skip(index) {
        *error = None;
      }
    }
    self.focus_changed.send_modify(|v| *v = v.wrapping_add(1));
    state.pending = (index..end)
      .filter(|i| !state.done[*i] && !state.leased[*i])
      .collect();
    drop(state);
    self.notify();
  }

  pub(crate) fn peer_status(&self, exhausted: bool, error: Option<String>) {
    let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(d) = &mut state.demand {
      d.peers_exhausted = exhausted;
      if let Some(error) = error {
        d.source_error = Some(error);
      }
    }
    drop(state);
    self.notify();
  }

  pub(crate) fn cdn_status(&self, result: Result<(), String>) {
    let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(d) = &mut state.demand {
      match result {
        Ok(()) => d.cdn_ready = true,
        Err(e) => {
          d.cdn_ready = false;
          d.source_error = Some(e);
        }
      }
    }
    drop(state);
    self.notify();
  }

  pub(crate) fn fail(&self, message: String) {
    if let Some(d) = &mut self.state.lock().unwrap_or_else(|e| e.into_inner()).demand {
      d.fatal = Some(message);
    }
    progress::finish(&self.progress, progress::TransferState::Failed);
    self.notify();
  }

  pub(crate) fn block_failed(&self, index: usize, error: String) {
    if let Some(d) = &mut self.state.lock().unwrap_or_else(|e| e.into_inner()).demand {
      d.errors[index] = Some(error);
    }
    self.notify();
  }

  pub(crate) async fn verified(&self, index: usize) -> std::io::Result<()> {
    let mut changed = self.changed.subscribe();
    loop {
      {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let d = state.demand.as_ref().expect("demand queue");
        if let Some(error) = &d.fatal {
          return Err(std::io::Error::other(error.clone()));
        }
        if state.done[index] {
          return Ok(());
        }
        if d.peers_exhausted
          && let Some(error) = d.errors[index]
            .as_ref()
            .or(d.source_error.as_ref().filter(|_| !d.cdn_ready))
        {
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
        let d = state.demand.as_ref().expect("demand queue");
        if state.remaining == 0 || (d.peers_exhausted && !state.pending.is_empty()) {
          return;
        }
        d.urgent
          .filter(|(i, _)| !state.done[*i])
          .map(|(_, deadline)| deadline)
      };
      if let Some(deadline) = deadline {
        tokio::select! { _ = tokio::time::sleep_until(deadline) => return, _ = changed.changed() => {} }
      } else {
        changed.changed().await.expect("queue sender");
      }
    }
  }

  pub(crate) async fn claim_cdn(self: &Arc<Self>) -> Option<BlockLease> {
    let mut changed = self.changed.subscribe();
    loop {
      let deadline = {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.remaining == 0 {
          return None;
        }
        let d = state.demand.as_ref().expect("demand queue");
        if d.fatal.is_some() {
          return None;
        }
        let deadline = d
          .urgent
          .filter(|(i, _)| !state.done[*i])
          .map(|(_, deadline)| deadline);
        let position = state.pending.iter().position(|i| {
          d.errors[*i].is_none()
            && (d.peers_exhausted
              || d.urgent.is_some_and(|(index, deadline)| {
                index == *i && tokio::time::Instant::now() >= deadline
              }))
        });
        if let Some(index) = position.and_then(|p| state.pending.remove(p)) {
          state.leased[index] = true;
          return Some(BlockLease {
            index,
            queue: self.clone(),
            committed: false,
          });
        }
        deadline.filter(|deadline| *deadline > tokio::time::Instant::now())
      };
      if let Some(deadline) = deadline {
        tokio::select! { _ = tokio::time::sleep_until(deadline) => {}, _ = changed.changed() => {} }
      } else {
        changed.changed().await.expect("queue sender");
      }
    }
  }

  pub(crate) async fn guard_peer<F: Future<Output = Result<T, TransferError>>, T>(
    &self,
    index: usize,
    operation: F,
  ) -> Result<T, TransferError> {
    tokio::pin!(operation);
    let mut changed = self.changed.subscribe();
    loop {
      let deadline = {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state
          .demand
          .as_ref()
          .filter(|d| d.cdn_ready && d.errors[index].is_none())
          .and_then(|d| d.urgent)
          .filter(|(i, _)| *i == index)
          .map(|(_, deadline)| deadline)
      };
      if let Some(deadline) = deadline {
        tokio::select! { result = &mut operation => return result, _ = tokio::time::sleep_until(deadline) => return Err(TransferError::Timeout), _ = changed.changed() => {} }
      } else {
        tokio::select! { result = &mut operation => return result, _ = changed.changed() => {} }
      }
    }
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
    let mut changed = self.changed.subscribe();
    loop {
      {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.demand.as_ref().is_some_and(|d| d.fatal.is_some()) {
          return None;
        }
        let eligible = state.pending.iter().position(|index| {
          !state.demand.as_ref().is_some_and(|d| {
            d.cdn_ready
              && d.errors[*index].is_none()
              && d
                .urgent
                .is_some_and(|(i, deadline)| i == *index && tokio::time::Instant::now() >= deadline)
          })
        });
        if let Some(index) = eligible.and_then(|position| state.pending.remove(position)) {
          state.leased[index] = true;
          return Some(BlockLease {
            index,
            queue: self.clone(),
            committed: false,
          });
        }
        if state.remaining == 0 {
          return None;
        }
      }
      changed.changed().await.expect("queue retains its sender");
    }
  }

  pub(crate) async fn finished(&self) {
    let mut changed = self.changed.subscribe();
    while {
      let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
      state.remaining != 0 && state.demand.as_ref().is_none_or(|d| d.fatal.is_none())
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
      if state
        .demand
        .as_ref()
        .is_none_or(|d| self.index >= d.start && self.index < d.end)
      {
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

/// Each candidate is tried once. Worker cancellation drops both its RTC session
/// and its lease; the JoinSet never detaches network workers.
pub(crate) async fn download_scoped<T, F, Fut>(
  targets: Vec<T>,
  seed: Seed,
  store: Arc<Mutex<DownloadStore>>,
  concurrency: usize,
  progress: watch::Sender<TransferProgress>,
  connect: F,
  scope: TaskScope,
) -> Result<(), TransferError>
where
  T: Send + 'static,
  F: Fn(T) -> Fut + Send + Sync + 'static,
  Fut: Future<Output = Result<PeerSession, TransferError>> + Send,
{
  let bits = store
    .lock()
    .map_err(|_| TransferError::Protocol("destination lock poisoned".into()))?
    .verified_bitfield();
  let queue = WorkQueue::scoped(&seed, &bits, progress, scope);
  let seed = Arc::new(seed);
  let connect = Arc::new(connect);
  let mut targets = targets.into_iter();
  let mut workers = JoinSet::new();
  let mut last_error = TransferError::NoPeers;
  loop {
    while workers.len() < concurrency && !queue.complete() {
      let Some(target) = targets.next() else {
        break;
      };
      let connect = connect.clone();
      let queue = queue.clone();
      let seed = seed.clone();
      let store = store.clone();
      let task = queue.scope.enter();
      workers.spawn(async move {
        let _task = task;
        let peer = tokio::select! {
          result = tokio::time::timeout(Duration::from_secs(45), connect(target)) =>
            result.map_err(|_| TransferError::Timeout)??,
          _ = queue.finished() => return Ok(()),
        };
        peer.download_scheduled(&seed, store, queue).await
      });
    }
    let Some(result) = workers.join_next().await else {
      break;
    };
    match result? {
      Ok(()) => {}
      Err(error) if error.peer_retryable() => last_error = error,
      Err(error) => return Err(error),
    }
  }
  if destination_complete(&store)? {
    Ok(())
  } else {
    Err(last_error)
  }
}

#[cfg(test)]
#[path = "../tests/scheduler.rs"]
mod tests;

#[cfg(test)]
pub(crate) use tests::download;
