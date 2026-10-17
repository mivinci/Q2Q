use super::*;

#[tokio::test]
async fn stop_retains_upload_slot_until_blocking_read_finishes() {
  let permits = Arc::new(Semaphore::new(1));
  let mut pool = UploadPool::new(
    permits.clone(),
    None,
    crate::upload_progress::Progress::new(),
  );
  let context = UploadContext::managed(
    pool.scope.clone(),
    Arc::new(permits.clone().try_acquire_owned().unwrap()),
    None,
    None,
  );
  let disk = context.disk_task();
  let (ready, started) = tokio::sync::oneshot::channel();
  let (release, waiting) = std::sync::mpsc::channel();
  pool.workers.spawn(async move {
    let _context = context;
    tokio::task::spawn_blocking(move || {
      let _disk = disk;
      ready.send(()).unwrap();
      waiting.recv().unwrap();
    })
    .await?;
    Ok(())
  });
  started.await.unwrap();
  let stop = pool.stop();
  tokio::pin!(stop);
  assert!(
    tokio::time::timeout(Duration::from_millis(10), &mut stop)
      .await
      .is_err()
  );
  assert_eq!(permits.available_permits(), 0);
  release.send(()).unwrap();
  tokio::time::timeout(Duration::from_secs(1), stop)
    .await
    .unwrap();
  assert_eq!(permits.available_permits(), 1);
}

#[tokio::test]
async fn stopping_rate_waiters_releases_slots_without_waiting_for_budget() {
  let permits = Arc::new(Semaphore::new(1));
  let rate = Arc::new(crate::upload_rate::UploadRate::new(1));
  rate.acquire(1041).await;
  let progress = crate::upload_progress::Progress::new();
  let connection = progress.connection(
    "waiting".into(),
    q2q_core::identity::PeerId::unchecked("remote"),
  );
  let mut pool = UploadPool::new(permits.clone(), Some(rate.clone()), progress.clone());
  let context = UploadContext::managed(
    pool.scope.clone(),
    Arc::new(permits.clone().try_acquire_owned().unwrap()),
    Some(rate.clone()),
    Some(connection),
  );
  let (ready, started) = tokio::sync::oneshot::channel();
  pool.workers.spawn(async move {
    let _context = context;
    ready.send(()).unwrap();
    rate.acquire(1041).await;
    Ok(())
  });
  started.await.unwrap();
  tokio::time::timeout(Duration::from_secs(1), pool.stop())
    .await
    .unwrap();
  assert_eq!(permits.available_permits(), 1);
  assert_eq!(progress.snapshot().cancelled_connections, 1);
  assert!(progress.snapshot().connections.is_empty());
}

#[tokio::test]
async fn cancelling_worker_before_first_poll_removes_its_progress_record() {
  let progress = Progress::new();
  let mut pool = UploadPool::new(Arc::new(Semaphore::new(1)), None, progress.clone());
  let connection = progress.connection(
    "not-started".into(),
    q2q_core::identity::PeerId::unchecked("remote"),
  );
  let task = pool.scope.enter();
  pool.workers.spawn(async move {
    let _connection = connection;
    let _task = task;
    std::future::pending::<Result<(), TransferError>>().await
  });
  pool.stop().await;
  let snapshot = progress.snapshot();
  assert!(snapshot.connections.is_empty());
  assert_eq!(snapshot.cancelled_connections, 1);
  assert_eq!(snapshot.handshaking_connections, 0);
}
