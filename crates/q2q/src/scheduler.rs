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
}

impl WorkQueue {
  pub(crate) fn new(
    seed: &Seed,
    bits: &[u8],
    progress: watch::Sender<TransferProgress>,
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
pub(crate) async fn download<T, F, Fut>(
  targets: Vec<T>,
  seed: Seed,
  store: Arc<Mutex<DownloadStore>>,
  concurrency: usize,
  progress: watch::Sender<TransferProgress>,
  connect: F,
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
  let queue = WorkQueue::new(&seed, &bits, progress);
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
      workers.spawn(async move {
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
mod tests {
  use super::*;
  use q2q_core::seed::Info;

  #[tokio::test]
  async fn exclusive_leases_requeue_on_drop_and_wake_idle_workers() {
    let seed = Seed::new(Info::new(262144 + 7, [0; 32]));
    let progress = progress::channel(&seed, &[]);
    let queue = WorkQueue::new(&seed, &[], progress.clone());
    let first = queue.claim().await.unwrap();
    let second = queue.claim().await.unwrap();
    assert_ne!(first.index, second.index);
    let idle = tokio::spawn({
      let queue = queue.clone();
      async move { queue.claim().await }
    });
    second.commit();
    assert!(!queue.complete());
    let abandoned = first.index;
    drop(first);
    let reassigned = tokio::time::timeout(Duration::from_secs(1), idle)
      .await
      .unwrap()
      .unwrap()
      .unwrap();
    assert_eq!(reassigned.index, abandoned);
    reassigned.commit();
    assert!(queue.claim().await.is_none());
    assert!(queue.complete());
    assert_eq!(queue.bitfield(), vec![0xc0]);
    assert_eq!(progress.borrow().verified_bytes, seed.info.length);
    assert_eq!(progress.borrow().verified_blocks, 2);
  }

  #[tokio::test]
  async fn resumed_blocks_are_never_leased_or_counted_twice() {
    let seed = Seed::new(Info::new(262144 + 7, [0; 32]));
    let progress = progress::channel(&seed, &[0x80]);
    let queue = WorkQueue::new(&seed, &[0x80], progress.clone());
    let lease = queue.claim().await.unwrap();
    assert_eq!(lease.index, 1);
    assert_eq!(progress.borrow().verified_bytes, 262144);
    lease.commit();
    assert!(queue.claim().await.is_none());
    assert_eq!(progress.borrow().verified_bytes, seed.info.length);
  }
}
