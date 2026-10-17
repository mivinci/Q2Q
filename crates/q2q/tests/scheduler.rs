use super::*;
use crate::progress;

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
