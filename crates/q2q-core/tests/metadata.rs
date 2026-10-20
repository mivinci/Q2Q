use super::*;
use crate::{
  peer::test_pair,
  peer_transfer::{receive, send},
  source_file::PreparedFile,
  upload_session::UploadContext,
};
use std::sync::Arc;

fn magnet(seed: &Seed) -> Magnet {
  q2q_primitives::seed::parse_magnet(&q2q_primitives::seed::to_magnet(seed)).unwrap()
}

#[tokio::test]
async fn metadata_only_exchange_closes_without_creating_a_download() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("source");
  std::fs::write(&path, b"verified metadata source").unwrap();
  let prepared = PreparedFile::open(path, "http://localhost".into()).unwrap();
  let expected = prepared.seed().info.clone();
  let (receiver, sender) = test_pair(expected.info_hash()).await;
  let (info, served) = tokio::time::timeout(Duration::from_secs(5), async {
    tokio::join!(
      async {
        let info = receiver.request_info().await.unwrap();
        receiver.close().await.unwrap();
        info
      },
      sender.serve_file_scoped(Arc::new(prepared), UploadContext::unmanaged())
    )
  })
  .await
  .unwrap();
  served.unwrap();
  assert_eq!(info.info, expected);
  assert!(info.cdn_list.is_empty());
  assert!(!dir.path().join("checkpoints").exists());
}

#[tokio::test]
async fn metadata_rejects_wrong_identity_oversize_and_old_peer_response() {
  let info = Info::new(0, q2q_primitives::merkle::EMPTY_ROOT);
  for (hash, message) in [
    (
      [9; 32],
      Control::Info {
        data: B64.encode(info.encode()),
      },
    ),
    (
      info.info_hash(),
      Control::Info {
        data: B64.encode(vec![0; MAX_INFO_BYTES + 1]),
      },
    ),
    (
      info.info_hash(),
      Control::Bitfield {
        bits: String::new(),
      },
    ),
    (info.info_hash(), Control::Info { data: "!!!".into() }),
  ] {
    let (receiver, sender) = test_pair(hash).await;
    let (result, _) = tokio::time::timeout(Duration::from_secs(5), async {
      tokio::join!(
        async {
          let result = receiver.request_info().await;
          receiver.close().await.unwrap();
          result
        },
        async {
          assert!(matches!(
            receive(&*sender.control).await.unwrap(),
            Control::RequestInfo {}
          ));
          send(&*sender.control, message).await.unwrap();
          let _ = receive(&*sender.control).await;
        }
      )
    })
    .await
    .unwrap();
    assert!(result.is_err());
  }
}

#[tokio::test]
async fn cancellation_closes_the_metadata_peer() {
  let (receiver, sender) = test_pair([9; 32]).await;
  let task = tokio::spawn(async move {
    let result = receiver.request_info().await;
    receiver.close().await.unwrap();
    result
  });
  assert!(matches!(
    receive(&*sender.control).await.unwrap(),
    Control::RequestInfo {}
  ));
  task.abort();
  assert!(task.await.unwrap_err().is_cancelled());
  assert!(
    tokio::time::timeout(Duration::from_secs(5), receive(&*sender.control))
      .await
      .unwrap()
      .is_err()
  );
}

#[tokio::test]
async fn cache_is_bounded_reverified_and_strips_remote_hints() {
  let dir = tempfile::tempdir().unwrap();
  let seed = Seed::new(Info::new(0, q2q_primitives::merkle::EMPTY_ROOT));
  let magnet = magnet(&seed);
  assert!(cached_magnet(&magnet, dir.path()).await.unwrap().is_none());
  store_cache(dir.path(), &magnet, &seed).await.unwrap();
  assert_eq!(
    cached_magnet(&magnet, dir.path()).await.unwrap(),
    Some(seed.clone())
  );
  let path = cache_path(dir.path(), &magnet);
  let mut with_hints = seed.clone();
  with_hints
    .cdn_list
    .push("https://untrusted.invalid/data?secret=not-a-credential".into());
  std::fs::write(&path, with_hints.encode()).unwrap();
  assert_eq!(
    cached_magnet(&magnet, dir.path()).await.unwrap(),
    Some(seed)
  );
  for bad in [
    b"corrupt".to_vec(),
    vec![0; MAX_INFO_BYTES + 100],
    Seed::new(Info::new(1, [4; 32])).encode(),
  ] {
    std::fs::write(&path, bad).unwrap();
    assert!(cached_magnet(&magnet, dir.path()).await.unwrap().is_none());
  }
  std::fs::remove_file(&path).unwrap();
  std::fs::create_dir(&path).unwrap();
  assert!(matches!(
    cached_magnet(&magnet, dir.path()).await,
    Err(MetadataError::Cache(_))
  ));
}

#[tokio::test]
async fn metadata_response_deadline_is_bounded() {
  let (receiver, sender) = test_pair([8; 32]).await;
  let task = tokio::spawn(async move {
    let result = receiver.request_info().await;
    receiver.close().await.unwrap();
    result
  });
  assert!(matches!(
    receive(&*sender.control).await.unwrap(),
    Control::RequestInfo {}
  ));
  tokio::time::pause();
  tokio::time::advance(Duration::from_secs(11)).await;
  assert!(matches!(task.await.unwrap(), Err(MetadataError::Timeout)));
}

#[tokio::test]
#[ignore = "requires fresh local Track/Punch; set Q2Q_TEST_TRACK_URL"]
async fn local_magnet_bad_peer_full_download_cache_and_partial_metadata() {
  tokio::time::timeout(Duration::from_secs(60), async {
    let track = std::env::var("Q2Q_TEST_TRACK_URL").expect("Q2Q_TEST_TRACK_URL");
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.bin");
    let bytes: Vec<_> = (0..700_001).map(|i| (i % 251) as u8).collect();
    std::fs::write(&source, &bytes).unwrap();
    let prepared = PreparedFile::open(source.clone(), track.clone()).unwrap();
    let magnet = magnet(prepared.seed());
    let hash = magnet.info_hash;
    let build = |name: &str| {
      crate::client::builder()
        .track_url(&track)
        .identity_file(dir.path().join(format!("{name}.identity")))
        .storage_dir(dir.path().join(name))
        .build()
    };
    let bad = build("bad").await.unwrap();
    let bad_session = bad.connect().await.unwrap();
    bad_session.update_files(&[hash], &[]).await.unwrap();
    let bad_serving = tokio::spawn({
      let session = bad_session.clone();
      async move {
        loop {
          // A healthy racing candidate may win during this handshake.
          let Ok(peer) = session.accept_peer(hash).await else { continue; };
          if matches!(receive(&*peer.control).await, Ok(Control::RequestInfo {})) {
            send(
              &*peer.control,
              Control::Info {
                data: B64.encode(Info::new(0, [9; 32]).encode()),
              },
            )
            .await
            .unwrap();
            let _ = receive(&*peer.control).await;
          }
        }
      }
    });
    let receiver = build("receiver").await.unwrap();
    let failure = receiver.resolve_magnet(&magnet).await.unwrap_err();
    assert!(matches!(failure, MetadataError::Exhausted { last, .. } if matches!(*last, MetadataError::Invalid(SeedError::InfoHashMismatch))));
    assert!(
      cached_magnet(&magnet, receiver.storage_dir())
        .await
        .unwrap()
        .is_none()
    );
    let publisher = build("publisher").await.unwrap();
    let publication = publisher.publish_file(&source).await.unwrap();
    let resolved = receiver.resolve_magnet(&magnet).await.unwrap();
    assert_eq!(resolved.info_hash(), hash);
    assert!(resolved.leaf_hashes.is_none() && resolved.cdn_list.is_empty());
    bad_session.update_files(&[], &[hash]).await.unwrap();
    bad_serving.abort();
    let _ = bad_serving.await;
    let output = dir.path().join("download.bin");
    receiver
      .download(&resolved, &output)
      .await
      .unwrap()
      .wait()
      .await
      .unwrap();
    assert_eq!(std::fs::read(&output).unwrap(), bytes);
    assert_eq!(publication.progress().failed_connections, 0);
    let partial = build("partial").await.unwrap();
    let store =
      crate::destination::Destination::open(&resolved, &dir.path().join("partial.bin"), false)
        .await
        .unwrap();
    let sharing = partial.share_destination(&store).await.unwrap();
    let bits = store.bitfield().await.unwrap();
    let queue = crate::block_queue::WorkQueue::new(&resolved, &bits, crate::progress::channel(&resolved, &bits));
    store.commit(queue.claim_available(&[0x80]).unwrap(), prepared.read_verified_block(0).unwrap(), prepared.proof(0).unwrap()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
      loop {
        let peers = receiver.connect().await.unwrap().query(&hash).await.unwrap().peers;
        if peers.iter().any(|peer| peer.peer_id == partial.peer_id().as_str() && !peer.complete) { break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
      }
    }).await.expect("partial holder was not announced");
    publication.close().await.unwrap();
    let cached = receiver.resolve_magnet(&magnet).await.unwrap();
    assert_eq!(cached.info_hash(), hash);
    std::fs::remove_file(cache_path(receiver.storage_dir(), &magnet)).unwrap();
    // A partial holder can supply info without being a complete data source.
    let from_partial = receiver.resolve_magnet(&magnet).await.unwrap();
    assert_eq!(from_partial.info_hash(), hash);
    sharing.close().await.unwrap();
    let resumed = crate::client::resume_offline(&from_partial, &output, receiver.storage_dir())
      .await
      .unwrap();
    let crate::client::OfflineResume::Complete(transfer) = resumed else {
      panic!("expected complete offline resume")
    };
    transfer.wait().await.unwrap();
    let no_source = Magnet {
      info_hash: [47; 32],
      display_name: None,
      trackers: vec![],
    };
    assert!(matches!(
      receiver.resolve_magnet(&no_source).await,
      Err(MetadataError::NoPeers)
    ));
  })
  .await
  .expect("magnet local scenario timed out");
}
