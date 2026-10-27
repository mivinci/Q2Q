//! Public SDK control-plane acceptance and a fully verified Workers Track P2P download.
use q2q_core::client::{self, QueryOptions, TransferState};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, path::Path, time::Duration};

async fn client(root: &Path, name: &str, track: &str) -> client::Client {
  client::builder()
    .track_url(track)
    .identity_file(root.join(format!("{name}.identity")))
    .storage_dir(root.join(name))
    .build()
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "run npm run test:sdk --workspace @q2q/track for isolated local services"]
async fn public_sdk_announces_heartbeats_paginates_and_recovers() {
  tokio::time::timeout(Duration::from_secs(30), acceptance())
    .await
    .expect("SDK control-plane acceptance timed out");
}

async fn acceptance() {
  let track = std::env::var("Q2Q_TEST_TRACK_URL").unwrap();
  let root = tempfile::tempdir().unwrap();
  let path = root.path().join("content");
  std::fs::write(&path, b"Workers Track public SDK compatibility").unwrap();
  let a = client(root.path(), "a", &track).await;
  let b = client(root.path(), "b", &track).await;
  let reader = client(root.path(), "reader", &track).await;
  let first = a.publish_file(&path).await.unwrap();
  let second = b.publish_file(&path).await.unwrap();
  let hash = first.seed().info_hash();
  assert_eq!(hash, second.seed().info_hash());
  let session = reader.connect().await.unwrap();
  session.heartbeat().await.unwrap();
  let mut cursor = None;
  let mut peers = HashSet::new();
  let mut pages = 0;
  loop {
    let page = session
      .query_with_options(
        &hash,
        QueryOptions {
          limit: Some(1),
          cursor,
        },
      )
      .await
      .unwrap();
    assert!(page.peers.len() <= 1);
    for peer in page.peers {
      assert!(peer.complete);
      assert!(peers.insert(peer.peer_id), "pagination repeated a peer");
    }
    pages += 1;
    assert!(pages <= 4, "pagination did not terminate");
    cursor = page.next_cursor;
    if cursor.is_none() {
      break;
    }
  }
  assert_eq!(
    peers,
    HashSet::from([a.peer_id().to_string(), b.peer_id().to_string()])
  );
  assert!(pages >= 2);

  // Fault injection is exposed only by the test Worker. The SDK itself is unchanged.
  if let Ok(expire) = std::env::var("Q2Q_TEST_EXPIRE_URL") {
    reqwest::Client::new()
      .post(expire)
      .json(&serde_json::json!({"peer_id": reader.peer_id().as_str()}))
      .send()
      .await
      .unwrap()
      .error_for_status()
      .unwrap();
    let error = session.heartbeat().await.unwrap_err();
    assert!(error.to_string().contains("unknown_session"), "{error}");
    // Existing public publish calls perform a fresh announce on this same session.
    // This tests explicit recovery; heartbeat/query do not silently reannounce.
    let recovered = reader.publish_file(&path).await.unwrap();
    session.heartbeat().await.unwrap();
    assert_eq!(session.query(&hash).await.unwrap().peers.len(), 2);
    recovered.close().await.unwrap();
  }
  first.close().await.unwrap();
  second.close().await.unwrap();
  assert!(session.query(&hash).await.unwrap().peers.is_empty());
  session.close().await.unwrap();
  a.connect().await.unwrap().close().await.unwrap();
  b.connect().await.unwrap().close().await.unwrap();
}

#[tokio::test]
#[ignore = "run npm run test:sdk --workspace @q2q/track for isolated local services"]
async fn public_sdk_downloads_p2p_and_verifies_full_file_hash() {
  tokio::time::timeout(Duration::from_secs(60), p2p())
    .await
    .expect("Workers Track P2P acceptance timed out");
}

async fn p2p() {
  let track = std::env::var("Q2Q_TEST_TRACK_URL").unwrap();
  let root = tempfile::tempdir().unwrap();
  let publisher = client(root.path(), "publisher", &track).await;
  let reader = client(root.path(), "downloader", &track).await;
  let bytes: Vec<u8> = (0..8 * 1024 * 1024 + 123)
    .map(|n| (n % 251) as u8)
    .collect();
  let original = root.path().join("original");
  std::fs::write(&original, &bytes).unwrap();
  let publication = publisher.publish_file(&original).await.unwrap();
  let info_hash = publication.info_hash();
  assert!(publication.seed().cdn_list.is_empty());
  assert!(publication.seed().proof_list.is_empty());
  let transfer = reader
    .download_with_track_discovery(publication.seed(), root.path().join("downloaded"))
    .await
    .unwrap();
  let progress = transfer.subscribe_progress();
  let path = transfer.wait().await.unwrap();
  let downloaded = std::fs::read(path).unwrap();
  assert_eq!(downloaded.len(), bytes.len());
  let expected = Sha256::digest(&bytes);
  assert_eq!(Sha256::digest(&downloaded), expected);
  assert_eq!(progress.borrow().state, TransferState::Completed);
  assert_eq!(progress.borrow().verified_bytes, bytes.len() as u64);
  assert!(publication.progress().sent_bytes >= bytes.len() as u64);
  println!(
    "P2P verified: bytes={} sha256={}",
    bytes.len(),
    expected
      .iter()
      .map(|byte| format!("{byte:02x}"))
      .collect::<String>()
  );
  publication.close().await.unwrap();
  assert!(
    reader
      .connect()
      .await
      .unwrap()
      .query(&info_hash)
      .await
      .unwrap()
      .peers
      .is_empty()
  );
  reader.connect().await.unwrap().close().await.unwrap();
  publisher.connect().await.unwrap().close().await.unwrap();
}
