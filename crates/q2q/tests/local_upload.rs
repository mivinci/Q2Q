//! Public API regression against local Track/Punch processes. No external Redis.
use q2q::client::{self, Client, ControlError, PeerSession, TransferError};
use q2q::publication::{PrepareError, Publication, PublicationState, UploadProgress};
use q2q_track::wire::PeerWire;
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

async fn client(root: &Path, name: &str, track: &str) -> Client {
  client::builder()
    .track_url(track)
    .identity_file(root.join(format!("{name}.identity")))
    .storage_dir(root.join(name))
    .max_upload_peers(2)
    .max_upload_reads(2)
    .upload_rate_limit(256 * 1024)
    .build()
    .await
    .unwrap()
}
async fn retry_busy(client: &Client, target: &PeerWire, hash: [u8; 32]) -> PeerSession {
  tokio::time::timeout(Duration::from_secs(5), async {
    loop {
      match client
        .connect()
        .await
        .unwrap()
        .dial_peer(target, hash)
        .await
      {
        Ok(peer) => return peer,
        Err(ControlError::Busy) => tokio::time::sleep(Duration::from_millis(100)).await,
        Err(error) => panic!("unexpected connection failure: {error}"),
      }
    }
  })
  .await
  .unwrap()
}

async fn settled(
  publication: &Publication,
  condition: impl Fn(&UploadProgress) -> bool,
) -> UploadProgress {
  tokio::time::timeout(Duration::from_secs(5), async {
    loop {
      let snapshot = publication.progress();
      if condition(&snapshot) {
        return snapshot;
      }
      tokio::time::sleep(Duration::from_millis(10)).await;
    }
  })
  .await
  .expect("upload progress did not settle")
}

#[tokio::test]
#[ignore = "requires local Track/Punch; set Q2Q_TEST_TRACK_URL and run with --ignored"]
async fn one_publisher_serves_parallel_receivers_with_bounded_admission_and_cleanup() {
  tokio::time::timeout(Duration::from_secs(45), scenario())
    .await
    .expect("concurrent upload scenario timed out");
}
async fn scenario() {
  let track = std::env::var("Q2Q_TEST_TRACK_URL").expect("Q2Q_TEST_TRACK_URL");
  let dir = tempfile::tempdir().unwrap();
  let source = dir
    .path()
    .join(format!("concurrent-{}.bin", std::process::id()));
  let bytes: Vec<_> = (0..2 * 1024 * 1024 + 13).map(|i| (i % 251) as u8).collect();
  std::fs::write(&source, &bytes).unwrap();
  let publisher = client(dir.path(), "publisher", &track).await;
  let a = client(dir.path(), "a", &track).await;
  let b = client(dir.path(), "b", &track).await;
  let c = client(dir.path(), "c", &track).await;
  let publication = publisher.publish_file(&source).await.unwrap();
  let updates = publication.subscribe_progress();
  assert_eq!(publication.progress().state, PublicationState::Publishing);
  let seed = publication.seed().clone();
  let hash = seed.info_hash();
  // Low-level accept must not reopen or consume the managed publisher's gate.
  let session = publisher.connect().await.unwrap();
  assert!(matches!(
    tokio::time::timeout(Duration::from_secs(1), session.accept_peer(hash))
      .await
      .unwrap(),
    Err(ControlError::AdmissionConflict)
  ));
  let a_session = a.connect().await.unwrap();
  let b_session = b.connect().await.unwrap();
  let c_session = c.connect().await.unwrap();
  let target = a_session
    .query(&hash)
    .await
    .unwrap()
    .peers
    .into_iter()
    .find(|peer| peer.peer_id == publisher.peer_id().as_str())
    .unwrap();
  // Concurrent complete handshakes while the first connection is still open.
  let (first, second) = tokio::join!(
    a_session.dial_peer(&target, hash),
    b_session.dial_peer(&target, hash)
  );
  let first = first.unwrap();
  let second = second.unwrap();
  assert!(matches!(
    c_session.dial_peer(&target, hash).await,
    Err(ControlError::Busy)
  ));
  let snapshot = settled(&publication, |p| p.idle_connections == 2).await;
  assert_eq!(snapshot.connections.len(), 2);
  assert_eq!(
    snapshot.failed_connections, 0,
    "Busy is not an admitted connection"
  );
  first.close().await.unwrap();
  let third = retry_busy(&c, &target, hash).await;
  let b_path = dir.path().join("b.bin");
  let c_path = dir.path().join("c.bin");
  let copying = tokio::time::Instant::now();
  let (left, right) = tokio::join!(
    async {
      let result = second.download_file(seed.clone(), &b_path).await;
      (result, copying.elapsed())
    },
    async {
      let result = third.download_file(seed.clone(), &c_path).await;
      (result, copying.elapsed())
    }
  );
  left.0.unwrap();
  right.0.unwrap();
  // Both ready receivers should make progress together, rather than letting
  // one finish at the single-connection rate (~8 seconds) before the other.
  assert!(
    left.1.min(right.1) >= Duration::from_secs(12),
    "unbalanced completion times: {:?}, {:?}",
    left.1,
    right.1
  );
  // Two 2 MiB files at a shared 256 KiB/s take about 16 seconds. A
  // mistakenly per-connection limiter would finish in about eight seconds.
  assert!(copying.elapsed() >= Duration::from_secs(15));
  assert_eq!(std::fs::read(&b_path).unwrap(), bytes);
  assert_eq!(std::fs::read(&c_path).unwrap(), bytes);
  assert!(
    !publication.is_finished(),
    "disconnecting one receiver stopped the source"
  );

  let snapshot = settled(&publication, |p| p.completed_connections == 2).await;
  assert_eq!(snapshot.state, PublicationState::Publishing);
  assert_eq!(snapshot.failed_connections, 1);
  assert!(snapshot.connections.is_empty());
  let expected = 2 * (bytes.len() as u64 + bytes.len().div_ceil(1024) as u64 * 17);
  assert_eq!(snapshot.sent_bytes - snapshot.retransmitted_bytes, expected);

  // Idle VOD connections retain their slots. Closing publication cancels both.
  let mut a_file = a
    .open(&seed, dir.path().join("a-vod"))
    .prefetch_blocks(0)
    .await
    .unwrap();
  let mut b_file = b
    .open(&seed, dir.path().join("b-vod"))
    .prefetch_blocks(0)
    .await
    .unwrap();
  let mut first_a = [0];
  let mut first_b = [0];
  let (left, right) = tokio::join!(
    a_file.read_exact(&mut first_a),
    b_file.read_exact(&mut first_b)
  );
  left.unwrap();
  right.unwrap();
  assert_eq!(a_file.progress().verified_blocks, 1);
  assert_eq!(b_file.progress().verified_blocks, 1);
  for file in [&a_file, &b_file] {
    let peers = file.peer_progress();
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0].peer_id, *publisher.peer_id());
    assert!(peers[0].connection_id.is_some());
    assert!(peers[0].received_bytes >= 262144);
    assert_eq!(peers[0].verified_bytes, 262144);
  }
  let a_peer_updates = a_file.subscribe_peer_progress();
  let b_peer_updates = b_file.subscribe_peer_progress();
  // Two VOD readers jump independently across different blocks, including the
  // short tail block. Neither connection shares or mutates a source cursor.
  for (left, right) in [(7, 3), (2, 6), (8, 4), (1, 5)] {
    a_file
      .seek(std::io::SeekFrom::Start(left * 262144))
      .await
      .unwrap();
    b_file
      .seek(std::io::SeekFrom::Start(right * 262144))
      .await
      .unwrap();
    let (a_read, b_read) = tokio::join!(
      a_file.read_exact(&mut first_a),
      b_file.read_exact(&mut first_b)
    );
    a_read.unwrap();
    b_read.unwrap();
    assert_eq!(first_a[0], bytes[left as usize * 262144]);
    assert_eq!(first_b[0], bytes[right as usize * 262144]);
  }
  assert!(matches!(
    c_session.dial_peer(&target, hash).await,
    Err(ControlError::Busy)
  ));
  settled(&publication, |p| p.idle_connections == 2).await;
  tokio::time::timeout(Duration::from_secs(5), publication.close())
    .await
    .unwrap()
    .unwrap();
  let snapshot = updates.borrow().clone();
  assert_eq!(snapshot.state, PublicationState::Closed);
  assert!(snapshot.connections.is_empty());
  assert_eq!(snapshot.cancelled_connections, 2);
  assert_eq!(snapshot.completed_connections, 2);
  a_file
    .seek(std::io::SeekFrom::Start(6 * 262144))
    .await
    .unwrap();
  b_file
    .seek(std::io::SeekFrom::Start(7 * 262144))
    .await
    .unwrap();
  let (left, right) = tokio::join!(a_file.read(&mut first_a), b_file.read(&mut first_b));
  assert!(left.is_err() && right.is_err());
  drop(a_file);
  drop(b_file);
  assert!(a_peer_updates.borrow().is_empty());
  assert!(b_peer_updates.borrow().is_empty());

  // Keep a low-level accept future alive across a conflicting publish call.
  // Polling it through a borrowed timeout proves that it owns admission and
  // the queue lock, reproducing the old startup deadlock without detaching it.
  {
    let accepting = session.accept_peer(hash);
    tokio::pin!(accepting);
    assert!(
      tokio::time::timeout(Duration::from_millis(10), &mut accepting)
        .await
        .is_err()
    );
    assert!(matches!(
      tokio::time::timeout(Duration::from_secs(1), publisher.publish_file(&source))
        .await
        .unwrap(),
      Err(TransferError::Control(ControlError::AdmissionConflict))
    ));
    assert!(
      tokio::time::timeout(Duration::from_millis(10), &mut accepting)
        .await
        .is_err()
    );
  }

  // Republish proves the global permits and publication guard were released.
  let publication = publisher.publish_file(&source).await.unwrap();
  assert_eq!(publication.progress().sent_bytes, 0);
  let failed_updates = publication.subscribe_progress();
  let (first, second) = tokio::join!(
    a_session.dial_peer(&target, hash),
    b_session.dial_peer(&target, hash)
  );
  let first = first.unwrap();
  let second = second.unwrap();
  std::fs::write(&source, vec![255; bytes.len()]).unwrap();
  let (left, right) = tokio::join!(
    first.download_file(seed.clone(), dir.path().join("changed-a")),
    second.download_file(seed, dir.path().join("changed-b"))
  );
  assert!(left.is_err() && right.is_err());
  tokio::time::timeout(Duration::from_secs(5), async {
    while !publication.is_finished() {
      tokio::task::yield_now().await;
    }
  })
  .await
  .unwrap();
  assert!(matches!(
    publication.close().await,
    Err(TransferError::Prepare(PrepareError::SourceChanged))
  ));
  let snapshot = failed_updates.borrow().clone();
  assert_eq!(snapshot.state, PublicationState::Failed);
  assert!(snapshot.connections.is_empty());
  assert!(snapshot.failed_connections >= 1);
  assert_eq!(
    snapshot.failed_connections + snapshot.cancelled_connections,
    2
  );
  assert!(
    !a_session
      .query(&hash)
      .await
      .unwrap()
      .peers
      .iter()
      .any(|peer| peer.peer_id == publisher.peer_id().as_str())
  );
  // Drop publishes Closing immediately and the retained receiver sees cleanup.
  let publication = publisher.publish_file(&source).await.unwrap();
  let mut dropped = publication.subscribe_progress();
  drop(publication);
  tokio::time::timeout(Duration::from_secs(5), async {
    loop {
      let state = dropped.borrow_and_update().state;
      if state == PublicationState::Closed {
        break;
      }
      dropped.changed().await.unwrap();
    }
  })
  .await
  .unwrap();
  assert!(dropped.borrow().connections.is_empty());
}
