//! Bounded peer workers and interruption at source-policy deadlines.
pub(crate) use crate::block_queue::WorkQueue;
use crate::destination::Destination;
use crate::error::TransferError;
use crate::peer::PeerSession;
use crate::progress::TransferProgress;
pub(crate) use crate::task_scope::TaskScope;
use q2q_core::seed::Seed;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinSet;

pub(crate) async fn guard_peer<F: Future<Output = Result<T, TransferError>>, T>(
  queue: &WorkQueue,
  index: usize,
  operation: F,
) -> Result<T, TransferError> {
  tokio::pin!(operation);
  let mut changed = queue.subscribe();
  loop {
    if let Some(deadline) = queue.peer_deadline(index) {
      tokio::select! { result = &mut operation => return result, _ = tokio::time::sleep_until(deadline) => return Err(TransferError::Timeout), _ = changed.changed() => {} }
    } else {
      tokio::select! { result = &mut operation => return result, _ = changed.changed() => {} }
    }
  }
}

pub(crate) async fn download_peer(
  peer: PeerSession,
  seed: &Seed,
  store: Destination,
  queue: Arc<WorkQueue>,
) -> Result<(), TransferError> {
  let mut connection = peer.begin_download(seed, queue.bitfield()).await?;
  let _active = crate::progress::ActivePeer::new(queue.progress.clone());
  while let Some(lease) = queue.claim().await {
    let index = lease.index;
    let length = crate::progress::block_len(seed.info.length, seed.info.block_size, index) as usize;
    let (bytes, proof) = guard_peer(&queue, index, connection.fetch(index, length)).await?;
    store.commit(lease, bytes, proof).await?;
    connection.acknowledge(index).await?;
  }
  connection.finish().await
}

/// Each candidate is tried once. Worker cancellation drops both its RTC session
/// and its lease; the JoinSet never detaches network workers.
pub(crate) async fn download_scoped<T, F, Fut>(
  targets: Vec<T>,
  seed: Seed,
  store: Destination,
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
  let bits = store.bitfield().await?;
  let queue = WorkQueue::scoped(&seed, &bits, progress, scope);
  run_peers(
    targets,
    Arc::new(seed),
    store.clone(),
    queue,
    concurrency,
    connect,
  )
  .await?;
  if store.complete().await? {
    Ok(())
  } else {
    Err(TransferError::NoPeers)
  }
}

pub(crate) async fn run_peers<T, F, Fut>(
  targets: Vec<T>,
  seed: Arc<Seed>,
  store: Destination,
  queue: Arc<WorkQueue>,
  concurrency: usize,
  connect: F,
) -> Result<(), TransferError>
where
  T: Send + 'static,
  F: Fn(T) -> Fut + Send + Sync + 'static,
  Fut: Future<Output = Result<PeerSession, TransferError>> + Send,
{
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
  if store.complete().await? {
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
