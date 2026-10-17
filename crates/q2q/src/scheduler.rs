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
}

pub(crate) struct WorkQueue {
  state: Mutex<State>,
  changed: watch::Sender<u64>,
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
        pending,
        done,
      }),
      changed: watch::channel(0).0,
      progress,
      length: seed.info.length,
      block_size: seed.info.block_size,
      scope,
    })
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
        if let Some(index) = state.pending.pop_front() {
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

  async fn finished(&self) {
    let mut changed = self.changed.subscribe();
    while !self.complete() {
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
      self
        .queue
        .state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .pending
        .push_front(self.index);
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
