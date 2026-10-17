use super::*;

fn peer(id: &str) -> PeerId {
  PeerId::unchecked(id)
}

#[test]
fn lifecycle_counts_are_partitioned_and_finished_details_are_removed() {
  let progress = Progress::new();
  let a = progress.connection("a".into(), peer("alice"));
  let b = progress.connection("b".into(), peer("bob"));
  assert_eq!(progress.snapshot().handshaking_connections, 2);
  a.state(UploadConnectionState::Idle);
  b.state(UploadConnectionState::Sending);
  a.sent(1041, false);
  b.sent(30, true);
  let snapshot = progress.snapshot();
  assert_eq!(
    (
      snapshot.handshaking_connections,
      snapshot.active_connections,
      snapshot.idle_connections
    ),
    (0, 1, 1)
  );
  assert_eq!(
    (snapshot.sent_bytes, snapshot.retransmitted_bytes),
    (1071, 30)
  );
  assert_eq!(snapshot.connections[1].peer_id, peer("bob"));
  assert_eq!(snapshot.connections[1].retransmitted_bytes, 30);
  a.finish(Outcome::Completed);
  drop(a);
  b.finish(Outcome::Failed);
  drop(b);
  let snapshot = progress.snapshot();
  assert!(snapshot.connections.is_empty());
  assert_eq!(
    (
      snapshot.completed_connections,
      snapshot.failed_connections,
      snapshot.cancelled_connections
    ),
    (1, 1, 0)
  );
  assert_eq!(snapshot.sent_bytes, 1071);
  assert_eq!(snapshot.state, PublicationState::Publishing);
}

#[tokio::test(start_paused = true)]
async fn normal_updates_are_coalesced_and_latest_snapshot_is_always_readable() {
  let progress = Progress::new();
  let mut updates = progress.subscribe();
  let a = progress.connection("a".into(), peer("alice"));
  let publishing = progress.clone();
  let timer = tokio::spawn(async move { publishing.coalesce().await });
  updates.changed().await.unwrap(); // Initial timer tick flushes admission.
  updates.borrow_and_update();
  for _ in 0..1000 {
    a.sent(1041, false);
  }
  assert_eq!(progress.snapshot().sent_bytes, 1_041_000);
  assert!(!updates.has_changed().unwrap());
  tokio::time::advance(Duration::from_millis(100)).await;
  updates.changed().await.unwrap();
  assert_eq!(updates.borrow_and_update().sent_bytes, 1_041_000);
  tokio::time::advance(Duration::from_millis(100)).await;
  tokio::task::yield_now().await;
  assert!(
    !updates.has_changed().unwrap(),
    "no notifications without changes"
  );
  timer.abort();
}

#[test]
fn closing_and_terminal_snapshots_flush_pending_bytes_immediately() {
  let progress = Progress::new();
  let a = progress.connection("a".into(), peer("alice"));
  let mut updates = progress.subscribe();
  a.sent(1041, false);
  progress.closing();
  assert!(updates.has_changed().unwrap());
  assert_eq!(updates.borrow_and_update().state, PublicationState::Closing);
  assert_eq!(updates.borrow().sent_bytes, 1041);
  drop(a);
  progress.finish(false);
  let snapshot = updates.borrow_and_update().clone();
  assert_eq!(snapshot.state, PublicationState::Closed);
  assert_eq!(snapshot.cancelled_connections, 1);
  assert!(snapshot.connections.is_empty());
  progress.closing();
  progress.finish(true); // Terminal states cannot be overwritten by guard Drop.
  assert!(!updates.has_changed().unwrap());
}

#[tokio::test]
async fn concurrent_counters_and_local_cancellation_are_exact() {
  let progress = Progress::new();
  let mut workers = Vec::new();
  for id in 0..16 {
    let connection = progress.connection(id.to_string(), peer("remote"));
    workers.push(tokio::spawn(async move {
      for _ in 0..100 {
        connection.sent(1041, true);
        tokio::task::yield_now().await;
      }
      // Guard drop classifies a locally cancelled worker without double counts.
    }));
  }
  for worker in workers {
    worker.await.unwrap();
  }
  let snapshot = progress.snapshot();
  assert_eq!(snapshot.sent_bytes, 16 * 100 * 1041);
  assert_eq!(snapshot.retransmitted_bytes, snapshot.sent_bytes);
  assert_eq!(snapshot.cancelled_connections, 16);
  assert!(snapshot.connections.is_empty());
}

#[test]
fn no_subscriber_or_unread_subscriber_cannot_backpressure_updates() {
  let progress = Progress::new();
  let a = progress.connection("a".into(), peer("remote"));
  a.sent(1, false);
  progress.flush();
  let slow = progress.subscribe();
  for _ in 0..1000 {
    a.sent(1, false);
    progress.flush();
  }
  assert_eq!(slow.borrow().sent_bytes, 1001);
  drop(slow);
  a.sent(1, false);
  let fresh = progress.subscribe();
  assert_eq!(fresh.borrow().sent_bytes, 1002);
}

#[test]
fn task_guard_marks_failure_and_later_cleanup_still_reaches_subscribers() {
  let progress = Progress::new();
  let a = progress.connection("a".into(), peer("remote"));
  let mut updates = progress.subscribe();
  drop(progress.run_guard());
  assert_eq!(updates.borrow_and_update().state, PublicationState::Failed);
  drop(a);
  assert!(updates.has_changed().unwrap());
  let snapshot = updates.borrow_and_update().clone();
  assert_eq!(snapshot.cancelled_connections, 1);
  assert_eq!(snapshot.state, PublicationState::Failed);
  assert!(snapshot.connections.is_empty());
}

#[test]
fn completed_history_does_not_accumulate_live_records() {
  let progress = Progress::new();
  let mut previous = None;
  for _ in 0..1000 {
    let a = progress.connection("reused-id".into(), peer("remote"));
    drop(previous.take());
    assert_eq!(progress.snapshot().connections.len(), 1);
    a.finish(Outcome::Completed);
    previous = Some(a);
  }
  drop(previous);
  assert!(progress.snapshot().connections.is_empty());
  assert_eq!(progress.snapshot().completed_connections, 1000);
  assert_eq!(progress.snapshot().cancelled_connections, 0);
}
