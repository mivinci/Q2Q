//! Explicit three-node regression against a fresh local Track/Punch pair.
use q2q::client::{self, Client, TransferError};
use q2q::publication::{Publication, PublicationState};
use q2q_track::wire::PeerWire;
use std::{path::Path, time::Duration};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

async fn client(root: &Path, name: &str, track: &str, slow: bool) -> Client {
  let builder = client::builder()
    .track_url(track)
    .identity_file(root.join(format!("{name}.identity")))
    .storage_dir(root.join(name))
    .max_upload_peers(2)
    .max_upload_reads(1);
  let builder = if slow {
    builder.upload_rate_limit(256 * 1024)
  } else {
    builder
  };
  builder.build().await.unwrap()
}
async fn holding(client: &Client, hash: [u8; 32], peer: &str, complete: bool) -> PeerWire {
  tokio::time::timeout(Duration::from_secs(10), async {
    loop {
      let result = client.connect().await.unwrap().query(&hash).await.unwrap();
      if let Some(peer) = result
        .peers
        .into_iter()
        .find(|p| p.peer_id == peer && p.complete == complete)
      {
        return peer;
      }
      tokio::time::sleep(Duration::from_millis(100)).await;
    }
  })
  .await
  .expect("holding did not settle")
}
async fn withdrawn(client: &Client, hash: [u8; 32], peer: &str) {
  tokio::time::timeout(Duration::from_secs(5), async {
    loop {
      if client
        .connect()
        .await
        .unwrap()
        .query(&hash)
        .await
        .unwrap()
        .peers
        .iter()
        .all(|p| p.peer_id != peer)
      {
        return;
      }
      tokio::time::sleep(Duration::from_millis(20)).await;
    }
  })
  .await
  .expect("holding was not withdrawn")
}
async fn closed(publication: &Publication) {
  tokio::time::timeout(Duration::from_secs(5), async {
    while !publication.is_finished() {
      tokio::time::sleep(Duration::from_millis(20)).await;
    }
  })
  .await
  .unwrap();
}

#[tokio::test]
#[ignore = "requires fresh local Track/Punch; set Q2Q_TEST_TRACK_URL"]
async fn three_nodes_relay_partial_blocks_and_keep_seeding_after_download() {
  tokio::time::timeout(Duration::from_secs(40), relay())
    .await
    .unwrap();
}
async fn relay() {
  let track = std::env::var("Q2Q_TEST_TRACK_URL").unwrap();
  let dir = tempfile::tempdir().unwrap();
  let a = client(dir.path(), "a", &track, true).await;
  let b = client(dir.path(), "b", &track, false).await;
  let c = client(dir.path(), "c", &track, false).await;
  let bytes: Vec<_> = (0..6 * 262144 + 13).map(|i| (i % 251) as u8).collect();
  let path = dir.path().join("original");
  std::fs::write(&path, &bytes).unwrap();
  let origin = a.publish_file(&path).await.unwrap();
  let seed = origin.seed().clone();
  let hash = seed.info_hash();
  let (transfer, sharing) = b
    .download_shared(&seed, dir.path().join("b-file"))
    .await
    .unwrap();
  let target = holding(&c, hash, b.peer_id().as_str(), false).await;
  assert!(!target.complete);
  assert_eq!(sharing.seed(), &seed);
  let peer = c
    .connect()
    .await
    .unwrap()
    .dial_peer(&target, hash)
    .await
    .unwrap();
  // Dial B directly: this receiver cannot silently use A as its full source.
  let receive = peer.download_file(seed, dir.path().join("c-file"));
  let (received, completed) = tokio::join!(receive, transfer.wait());
  assert_eq!(std::fs::read(received.unwrap()).unwrap(), bytes);
  assert_eq!(std::fs::read(completed.unwrap()).unwrap(), bytes);
  holding(&c, hash, b.peer_id().as_str(), true).await;
  assert!(sharing.progress().sent_bytes >= bytes.len() as u64);
  assert!(!sharing.is_finished());
  sharing.close().await.unwrap();
  withdrawn(&c, hash, b.peer_id().as_str()).await;
  assert_eq!(origin.progress().state, PublicationState::Publishing);
  origin.close().await.unwrap();
}

#[tokio::test]
#[ignore = "requires fresh local Track/Punch; set Q2Q_TEST_TRACK_URL"]
async fn multiple_contents_route_independently_and_vod_cancellation_withdraws() {
  tokio::time::timeout(Duration::from_secs(40), contents())
    .await
    .unwrap();
}
async fn contents() {
  let track = std::env::var("Q2Q_TEST_TRACK_URL").unwrap();
  let dir = tempfile::tempdir().unwrap();
  let a = client(dir.path(), "a", &track, false).await;
  let b = client(dir.path(), "b", &track, false).await;
  let c = client(dir.path(), "c", &track, false).await;
  let first = dir.path().join("first");
  let second = dir.path().join("second");
  let bytes = vec![19; 3 * 262144 + 13];
  std::fs::write(&first, &bytes).unwrap();
  std::fs::write(&second, b"independent content").unwrap();
  let one = a.publish_file(&first).await.unwrap();
  let two = a.publish_file(&second).await.unwrap();
  assert!(matches!(
    a.publish_file(&first).await,
    Err(TransferError::AlreadyPublishing)
  ));
  let mut reader = b
    .open(one.seed(), dir.path().join("vod"))
    .prefetch_blocks(0)
    .share(true)
    .await
    .unwrap();
  let sharing = reader.take_sharing().unwrap();
  assert!(
    b.connect()
      .await
      .unwrap()
      .query(&one.info_hash())
      .await
      .unwrap()
      .peers
      .iter()
      .all(|p| p.peer_id != b.peer_id().as_str())
  );
  let mut block = vec![0; 262144];
  reader.read_exact(&mut block).await.unwrap();
  assert_eq!(block, bytes[..262144]);
  holding(&c, one.info_hash(), b.peer_id().as_str(), false).await;
  // Closing one hash leaves the same Client's other upload route active.
  one.close().await.unwrap();
  let result = c
    .download(two.seed(), dir.path().join("second-copy"))
    .await
    .unwrap()
    .wait()
    .await
    .unwrap();
  assert_eq!(std::fs::read(result).unwrap(), b"independent content");
  // Only B remains indexed for this hash: C must discover and use a partial peer.
  let mut downstream = c
    .open(sharing.seed(), dir.path().join("downstream"))
    .prefetch_blocks(0)
    .await
    .unwrap();
  downstream.read_exact(&mut block).await.unwrap();
  assert_eq!(block, bytes[..262144]);
  assert!(sharing.progress().sent_bytes >= 262144);
  // Sparse bytes beyond B's window are never exposed.
  downstream
    .seek(std::io::SeekFrom::Start(2 * 262144))
    .await
    .unwrap();
  drop(downstream);
  drop(reader);
  closed(&sharing).await;
  assert_eq!(sharing.progress().state, PublicationState::Failed);
  withdrawn(&c, sharing.info_hash(), b.peer_id().as_str()).await;
  assert!(sharing.close().await.is_err());
  two.close().await.unwrap();
}

#[tokio::test]
#[ignore = "requires fresh local Track/Punch; set Q2Q_TEST_TRACK_URL"]
async fn existing_vod_reader_discovers_a_later_partial_source() {
  tokio::time::timeout(Duration::from_secs(35), late_source())
    .await
    .unwrap();
}
async fn late_source() {
  let track = std::env::var("Q2Q_TEST_TRACK_URL").unwrap();
  let dir = tempfile::tempdir().unwrap();
  let a = client(dir.path(), "a", &track, false).await;
  let b = client(dir.path(), "b", &track, false).await;
  let c = client(dir.path(), "c", &track, false).await;
  let bytes = vec![41; 3 * 262144 + 13];
  let original = dir.path().join("original");
  std::fs::write(&original, &bytes).unwrap();
  let origin = a.publish_file(&original).await.unwrap();
  let seed = origin.seed().clone();
  let cache = dir.path().join("cache");
  let mut upstream = b.open(&seed, &cache).prefetch_blocks(0).await.unwrap();
  let mut block = vec![0; 262144];
  upstream.read_exact(&mut block).await.unwrap();
  origin.close().await.unwrap();
  drop(upstream);
  // This reader initially has no candidates. It must remain able to discover
  // a new partial holder without being reopened or changing the read window.
  let mut downstream = c
    .open(&seed, dir.path().join("downstream"))
    .prefetch_blocks(0)
    .await
    .unwrap();
  assert!(downstream.read_exact(&mut block).await.is_err());
  let upstream = b
    .open(&seed, &cache)
    .prefetch_blocks(0)
    .share(true)
    .await
    .unwrap();
  let sharing = upstream.sharing().unwrap();
  holding(&c, seed.info_hash(), b.peer_id().as_str(), false).await;
  tokio::time::sleep(Duration::from_secs(6)).await;
  downstream.read_exact(&mut block).await.unwrap();
  assert_eq!(block, bytes[..262144]);
  assert!(sharing.progress().sent_bytes >= 262144);
  drop(downstream);
  drop(upstream);
  withdrawn(&c, seed.info_hash(), b.peer_id().as_str()).await;
}

#[tokio::test]
#[ignore = "requires fresh local Track/Punch; set Q2Q_TEST_TRACK_URL"]
async fn closing_sharing_does_not_cancel_download() {
  tokio::time::timeout(Duration::from_secs(30), independent_close())
    .await
    .unwrap();
}
async fn independent_close() {
  let track = std::env::var("Q2Q_TEST_TRACK_URL").unwrap();
  let dir = tempfile::tempdir().unwrap();
  let a = client(dir.path(), "a", &track, true).await;
  let b = client(dir.path(), "b", &track, false).await;
  let c = client(dir.path(), "c", &track, false).await;
  let bytes = vec![71; 3 * 262144 + 13];
  let path = dir.path().join("original");
  std::fs::write(&path, &bytes).unwrap();
  let origin = a.publish_file(path).await.unwrap();
  let (transfer, sharing) = b
    .download_shared(origin.seed(), dir.path().join("download"))
    .await
    .unwrap();
  holding(&c, origin.info_hash(), b.peer_id().as_str(), false).await;
  sharing.close().await.unwrap();
  withdrawn(&c, origin.info_hash(), b.peer_id().as_str()).await;
  assert_eq!(
    std::fs::read(transfer.wait().await.unwrap()).unwrap(),
    bytes
  );
  withdrawn(&c, origin.info_hash(), b.peer_id().as_str()).await;
  let (cancelled, sharing) = b
    .download_shared(origin.seed(), dir.path().join("cancelled-download"))
    .await
    .unwrap();
  holding(&c, origin.info_hash(), b.peer_id().as_str(), false).await;
  cancelled.cancel();
  closed(&sharing).await;
  withdrawn(&c, origin.info_hash(), b.peer_id().as_str()).await;
  assert!(sharing.close().await.is_err());
  origin.close().await.unwrap();
}

#[tokio::test]
#[ignore = "requires fresh local Track/Punch; set Q2Q_TEST_TRACK_URL"]
async fn query_options_paginate_candidates_and_exclude_self() {
  use q2q::client::{ControlError, QueryOptions};
  let track = std::env::var("Q2Q_TEST_TRACK_URL").unwrap();
  let dir = tempfile::tempdir().unwrap();
  let a = client(dir.path(), "a", &track, false).await;
  let b = client(dir.path(), "b", &track, false).await;
  let c = client(dir.path(), "c", &track, false).await;
  let path = dir.path().join("shared");
  std::fs::write(&path, b"query candidates").unwrap();
  let one = a.publish_file(&path).await.unwrap();
  let two = b.publish_file(&path).await.unwrap();
  let three = c.publish_file(&path).await.unwrap();
  assert_eq!(one.info_hash(), two.info_hash());
  let session = a.connect().await.unwrap();
  let first = session
    .query_with_options(
      &one.info_hash(),
      QueryOptions {
        limit: Some(1),
        cursor: None,
      },
    )
    .await
    .unwrap();
  assert_eq!(first.peers.len(), 1);
  assert_ne!(first.peers[0].peer_id, a.peer_id().as_str());
  let second = session
    .query_with_options(
      &one.info_hash(),
      QueryOptions {
        limit: Some(1),
        cursor: Some(first.next_cursor.unwrap()),
      },
    )
    .await
    .unwrap();
  assert_eq!(second.peers.len(), 1);
  assert_ne!(first.peers[0].peer_id, second.peers[0].peer_id);
  assert!(second.next_cursor.is_none());
  let all = session
    .query_with_options(
      &one.info_hash(),
      QueryOptions {
        limit: Some(50),
        cursor: None,
      },
    )
    .await
    .unwrap();
  assert_eq!(all.peers.len(), 2);
  assert!(all.next_cursor.is_none());
  assert!(matches!(
    session
      .query_with_options(
        &one.info_hash(),
        QueryOptions {
          limit: Some(0),
          cursor: None
        }
      )
      .await,
    Err(ControlError::Protocol(_))
  ));
  two.close().await.unwrap();
  let remaining = session
    .query_with_options(
      &one.info_hash(),
      QueryOptions {
        limit: Some(8),
        cursor: None,
      },
    )
    .await
    .unwrap();
  assert_eq!(remaining.peers.len(), 1);
  assert_eq!(remaining.peers[0].peer_id, c.peer_id().as_str());
  three.close().await.unwrap();
  one.close().await.unwrap();
}
