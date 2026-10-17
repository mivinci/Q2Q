use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

#[tokio::test]
async fn active_handles_share_one_task_and_the_original_failure() {
  let online = Online::default();
  let calls = Arc::new(AtomicUsize::new(0));
  let (fail, failed) = tokio::sync::oneshot::channel();
  let count = calls.clone();
  let first = online.lease(async move {
    count.fetch_add(1, Ordering::SeqCst);
    failed.await.unwrap();
    ControlError::Protocol("session failed".into())
  });
  let second = online.lease(async { panic!("a second heartbeat task was started") });
  assert!(Arc::ptr_eq(&first, &second));
  fail.send(()).unwrap();
  let (a, b) = tokio::join!(first.failed(), second.failed());
  assert!(Arc::ptr_eq(&a, &b));
  assert!(matches!(&*a, ControlError::Protocol(message) if message == "session failed"));
  assert_eq!(calls.load(Ordering::SeqCst), 1);
  drop(first);
  drop(second);
  let fresh = online.lease(async { ControlError::Protocol("new session use".into()) });
  assert!(
    matches!(&*fresh.failed().await, ControlError::Protocol(message) if message == "new session use")
  );
}

#[tokio::test]
async fn the_last_handle_stops_heartbeat_and_releases_its_session() {
  struct Session(Arc<AtomicUsize>);
  impl Drop for Session {
    fn drop(&mut self) {
      self.0.fetch_add(1, Ordering::SeqCst);
    }
  }
  let dropped = Arc::new(AtomicUsize::new(0));
  let session = Session(dropped.clone());
  let online = Online::default();
  let first = online.lease(async move {
    let _session = session;
    std::future::pending().await
  });
  let second = first.clone();
  tokio::task::yield_now().await;
  drop(first);
  assert_eq!(dropped.load(Ordering::SeqCst), 0);
  drop(second);
  tokio::time::timeout(Duration::from_secs(1), async {
    while dropped.load(Ordering::SeqCst) == 0 {
      tokio::task::yield_now().await;
    }
  })
  .await
  .unwrap();
}

#[tokio::test]
async fn a_panicked_heartbeat_does_not_leave_its_users_waiting_forever() {
  let online = Online::default();
  let task = online.lease(async { panic!("heartbeat worker panic") });
  let error = tokio::time::timeout(Duration::from_secs(1), task.failed())
    .await
    .unwrap();
  assert!(
    matches!(&*error, ControlError::Protocol(message) if message.contains("heartbeat task ended"))
  );
}
