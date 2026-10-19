use super::*;
use crate::progress;
use crate::progress::TransferProgress;
use tokio::{sync::watch, task::JoinSet};

pub(crate) async fn download<T, F, Fut>(
  targets: Vec<T>,
  seed: Seed,
  store: Destination,
  concurrency: usize,
  progress: watch::Sender<TransferProgress>,
  connect: F,
) -> Result<(), TransferError>
where
  T: Send + 'static,
  F: Fn(T) -> Fut + Send + Sync + 'static,
  Fut: Future<Output = Result<PeerSession, TransferError>> + Send,
{
  download_scoped(
    targets,
    seed,
    store,
    concurrency,
    progress,
    connect,
    TaskScope::new(),
  )
  .await
}

use q2q_primitives::seed::Info;

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

#[test]
fn partial_claims_intersect_availability_and_keep_leases_exclusive() {
  let seed = Seed::new(Info::new(3 * 262144 + 7, [0; 32]));
  let queue = WorkQueue::new(&seed, &[0x80], progress::channel(&seed, &[0x80]));
  assert!(queue.claim_available(&[0x80]).is_none());
  let third = queue.claim_available(&[0xa0]).unwrap();
  assert_eq!(third.index, 2);
  assert!(queue.claim_available(&[0xa0]).is_none());
  let second = queue.claim_available(&[0x60]).unwrap();
  assert_eq!(second.index, 1);
  drop(third);
  let third = queue.claim_available(&[0x20]).unwrap();
  assert_eq!(third.index, 2);
  third.commit();
  second.commit();
  assert_eq!(queue.claim_available(&[0xff]).unwrap().index, 3);
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
