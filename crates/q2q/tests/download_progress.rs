use super::*;
use crate::{block_queue::WorkQueue, destination::Destination, source_file::PreparedFile};

fn peer(id: &str) -> PeerId {
  PeerId::unchecked(id)
}

#[test]
fn attempts_partition_bytes_and_reconnects_have_new_identities() {
  let progress = Progress::new();
  let a = progress.connection(peer("alice"));
  let b = progress.connection(peer("bob"));
  a.connected("conn-a", &peer("alice"));
  a.state(DownloadPeerState::Receiving);
  a.received(100);
  b.received(200);
  a.verified(80);
  let rows = progress.snapshot();
  assert_ne!(rows[0].attempt_id, rows[1].attempt_id);
  assert_eq!((rows[0].received_bytes, rows[0].verified_bytes), (100, 80));
  assert_eq!((rows[1].received_bytes, rows[1].verified_bytes), (200, 0));
  assert_eq!(rows[0].connection_id.as_deref(), Some("conn-a"));
  assert_eq!(rows[1].state, DownloadPeerState::Connecting);
  assert!(rows[1].connection_id.is_none());
  let old_id = rows[0].attempt_id;
  drop(a);
  let _reconnected = progress.connection(peer("alice"));
  let rows = progress.snapshot();
  assert_eq!(rows.len(), 2);
  assert!(rows[1].attempt_id > old_id);
  assert_eq!(rows[1].received_bytes, 0);
}

#[test]
fn close_is_terminal_and_late_guards_cannot_repopulate_rows() {
  let progress = Progress::new();
  let a = progress.connection(peer("alice"));
  let updates = progress.subscribe();
  progress.close();
  a.received(100);
  a.verified(80);
  a.connected("late", &peer("alice"));
  let _late = progress.connection(peer("bob"));
  drop(a);
  progress.close();
  assert!(progress.snapshot().is_empty());
  assert!(updates.borrow().is_empty());
}

#[tokio::test(start_paused = true)]
async fn pieces_update_snapshots_without_per_piece_watch_notifications() {
  let progress = Progress::new();
  let a = progress.connection(peer("alice"));
  let mut updates = progress.subscribe();
  let timer = tokio::spawn({
    let progress = progress.clone();
    async move { progress.coalesce().await }
  });
  tokio::task::yield_now().await;
  updates.borrow_and_update();
  for _ in 0..1000 {
    a.received(1024);
  }
  assert_eq!(progress.snapshot()[0].received_bytes, 1_024_000);
  assert!(!updates.has_changed().unwrap());
  tokio::time::advance(Duration::from_millis(100)).await;
  updates.changed().await.unwrap();
  assert_eq!(updates.borrow_and_update()[0].received_bytes, 1_024_000);
  drop(a);
  assert!(updates.borrow().is_empty());
  timer.abort();
}

#[test]
fn last_guard_removes_rows_without_retaining_history() {
  let progress = Progress::new();
  let connection = progress.connection(peer("alice"));
  let other_owner = connection.clone();
  drop(connection);
  assert_eq!(progress.snapshot().len(), 1);
  drop(other_owner);
  assert!(progress.snapshot().is_empty());
}

#[tokio::test]
async fn verified_attribution_requires_successful_durable_commit() {
  for failure in ["none", "proof", "receipt"] {
    let dir = tempfile::tempdir().unwrap();
    let source_path = dir.path().join("source");
    let bytes = vec![19; 1025];
    std::fs::write(&source_path, &bytes).unwrap();
    let source = PreparedFile::open(source_path, "http://localhost".into()).unwrap();
    let path = dir.path().join("destination");
    let store = Destination::open(source.seed(), &path, false)
      .await
      .unwrap();
    let queue = WorkQueue::new(
      source.seed(),
      &[],
      crate::progress::channel(source.seed(), &[]),
    );
    let connection = queue.peers.connection(peer("alice"));
    connection.received(bytes.len());
    let mut lease = queue.claim_available(&[0x80]).unwrap();
    lease.attribute(&connection);
    if failure == "receipt" {
      crate::download_store::tests::make_receipts_read_only(&mut store.lock().unwrap(), &path);
    }
    let mut received = bytes.clone();
    if failure == "proof" {
      received[0] ^= 1;
    }
    let result = store
      .commit(lease, received, source.proof(0).unwrap())
      .await;
    assert_eq!(result.is_ok(), failure == "none");
    let row = queue.peers.snapshot().pop().unwrap();
    assert_eq!(row.received_bytes, bytes.len() as u64);
    assert_eq!(
      row.verified_bytes,
      if failure == "none" {
        bytes.len() as u64
      } else {
        0
      }
    );
  }
}

#[tokio::test]
async fn unattributed_and_resumed_blocks_do_not_count_as_peer_bytes() {
  let dir = tempfile::tempdir().unwrap();
  let bytes = vec![20; 262144 + 7];
  let path = dir.path().join("source");
  std::fs::write(&path, &bytes).unwrap();
  let source = PreparedFile::open(path, "http://localhost".into()).unwrap();
  let store = Destination::open(source.seed(), &dir.path().join("destination"), false)
    .await
    .unwrap();
  let queue = WorkQueue::new(
    source.seed(),
    &[],
    crate::progress::channel(source.seed(), &[]),
  );
  let _connection = queue.peers.connection(peer("alice"));
  // CDN leases have no attribution even while a Peer is connected.
  let lease = queue.claim_available(&[0x80]).unwrap();
  store
    .commit(lease, bytes[..262144].to_vec(), source.proof(0).unwrap())
    .await
    .unwrap();
  assert_eq!(queue.peers.snapshot()[0].verified_bytes, 0);
  let bits = store.bitfield().await.unwrap();
  let resumed = WorkQueue::new(
    source.seed(),
    &bits,
    crate::progress::channel(source.seed(), &bits),
  );
  let _connection = resumed.peers.connection(peer("alice"));
  assert_eq!(resumed.progress.borrow().verified_bytes, 262144);
  assert_eq!(resumed.peers.snapshot()[0].verified_bytes, 0);
}

#[tokio::test]
async fn cancelling_transfer_closes_observers_before_aborting_workers() {
  let seed = q2q_core::seed::Seed::new(q2q_core::seed::Info::new(0, q2q_core::merkle::EMPTY_ROOT));
  let peers = Progress::new();
  let connection = peers.connection(peer("alice"));
  let transfer = crate::transfer::Transfer::observed(
    crate::progress::channel(&seed, &[]),
    peers.clone(),
    std::future::pending(),
  );
  let updates = transfer.subscribe_peer_progress();
  transfer.cancel();
  connection.received(1024);
  assert!(updates.borrow().is_empty());
  assert!(peers.snapshot().is_empty());
}

#[tokio::test]
async fn success_and_failure_publish_empty_peer_snapshots() {
  for fail in [false, true] {
    let seed =
      q2q_core::seed::Seed::new(q2q_core::seed::Info::new(0, q2q_core::merkle::EMPTY_ROOT));
    let peers = Progress::new();
    let _connection = peers.connection(peer("alice"));
    let transfer = crate::transfer::Transfer::observed(
      crate::progress::channel(&seed, &[]),
      peers,
      async move {
        if fail {
          Err(crate::error::TransferError::Closed)
        } else {
          Ok("output".into())
        }
      },
    );
    let updates = transfer.subscribe_peer_progress();
    assert_eq!(transfer.wait().await.is_err(), fail);
    assert!(updates.borrow().is_empty());
  }
}
