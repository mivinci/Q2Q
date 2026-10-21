use super::*;
use crate::test_http::*;
use crate::{block_queue::WorkQueue, download::cdn_window as cdn, progress};
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn demand_seek_discards_old_pending_but_retains_inflight() {
  let seed = fixture(&vec![1; 4 * 262144]);
  let queue = WorkQueue::demand(&seed, &[], progress::channel(&seed, &[]));
  queue.focus(0, 2, Duration::from_secs(1), true);
  let old = queue.claim().await.unwrap();
  assert_eq!(old.index, 0);
  queue.focus(3, 0, Duration::from_secs(1), true);
  drop(old);
  let next = queue.claim().await.unwrap();
  assert_eq!(next.index, 3);
  next.commit();
  assert!(
    tokio::time::timeout(Duration::from_millis(10), queue.claim())
      .await
      .is_err()
  );
  queue.focus(0, 0, Duration::ZERO, true);
  assert_eq!(queue.claim().await.unwrap().index, 0);
}

#[tokio::test]
async fn urgent_fallback_releases_only_its_peer_lease() {
  let seed = fixture(&vec![1; 3 * 262144]);
  let queue = WorkQueue::demand(&seed, &[], progress::channel(&seed, &[]));
  queue.focus(0, 1, Duration::from_millis(15), true);
  let slow = queue.claim().await.unwrap();
  let other = queue.claim().await.unwrap();
  assert_eq!((slow.index, other.index), (0, 1));
  queue.cdn_status(Ok(()));
  let guarded = crate::scheduler::guard_peer(
    &queue,
    0,
    std::future::pending::<Result<(), TransferError>>(),
  );
  assert!(matches!(guarded.await, Err(TransferError::Timeout)));
  // A CDN worker must still wait until the old owner releases the block.
  assert!(
    tokio::time::timeout(Duration::from_millis(10), queue.claim_cdn())
      .await
      .is_err()
  );
  drop(slow);
  let cdn = queue.claim_cdn().await.unwrap();
  assert_eq!(cdn.index, 0);
  queue.focus(2, 0, Duration::from_secs(1), true);
  // Existing peer can finish its old block while CDN independently completes 0.
  other.commit();
  cdn.commit();
  assert_eq!(queue.progress.borrow().verified_blocks, 2);
  assert_eq!(queue.claim().await.unwrap().index, 2);
}

#[tokio::test]
async fn healthy_p2p_window_does_not_fetch_cdn_metadata() {
  let dir = tempfile::tempdir().unwrap();
  let bytes = vec![37; 2 * 262144 + 5];
  let source = dir.path().join("source");
  std::fs::write(&source, &bytes).unwrap();
  let prepared = crate::publication::PreparedFile::open(source, "http://localhost".into()).unwrap();
  let mut seed = prepared.seed().clone();
  let server = Server::start(|_| panic!("healthy P2P must not contact CDN")).await;
  seed.cdn_list = vec![server.url.clone()];
  let store =
    crate::test_destination::open_destination(&seed, &dir.path().join("destination"), false)
      .await
      .unwrap();
  let queue = WorkQueue::demand(&seed, &[], progress::channel(&seed, &[]));
  let (download, upload) = crate::peer::test_pair(seed.info_hash()).await;
  let upload = tokio::spawn(async move { upload.serve_file(Arc::new(Mutex::new(prepared))).await });
  let cdn_task = tokio::spawn(cdn(
    Arc::new(seed.clone()),
    store.clone(),
    queue.clone(),
    2,
    q2q_http::Executor::builder().build().unwrap(),
  ));
  let peer_task = tokio::spawn({
    let queue = queue.clone();
    let store = store.clone();
    let seed = seed.clone();
    async move { download.download_scheduled(&seed, store, queue).await }
  });
  for index in [2, 0, 1] {
    queue.focus(index, 0, Duration::from_secs(2), true);
    tokio::time::timeout(Duration::from_secs(2), queue.verified(index))
      .await
      .unwrap()
      .unwrap();
  }
  peer_task.await.unwrap().unwrap();
  upload.await.unwrap().unwrap();
  cdn_task.await.unwrap();
  assert!(server.requests.lock().unwrap().is_empty());
  assert_eq!(
    std::fs::read(dir.path().join("destination")).unwrap(),
    bytes
  );
}

#[tokio::test]
async fn actual_peer_fallback_keeps_other_peer_alive() {
  use base64::Engine as _;
  use q2q_primitives::peer_wire::Control;
  use webrtc::data_channel::DataChannelEvent;
  async fn receive(peer: &crate::peer::PeerSession) -> Control {
    loop {
      match peer.control.poll().await {
        Some(DataChannelEvent::OnMessage(message)) => {
          return Control::decode(&message.data).unwrap();
        }
        Some(DataChannelEvent::OnClose) | None => panic!("unexpected close"),
        _ => {}
      }
    }
  }
  let dir = tempfile::tempdir().unwrap();
  let bytes = vec![67; 3 * 262144];
  let path = dir.path().join("source");
  std::fs::write(&path, &bytes).unwrap();
  let prepared = crate::publication::PreparedFile::open(path, "http://localhost".into()).unwrap();
  let mut seed = prepared.seed().clone();
  let served = bytes.clone();
  let server = Server::start(move |request| range_reply(&request, &served)).await;
  seed.cdn_list = vec![server.url.clone()];
  let store =
    crate::test_destination::open_destination(&seed, &dir.path().join("destination"), false)
      .await
      .unwrap();
  let queue = WorkQueue::demand(&seed, &[], progress::channel(&seed, &[]));
  let (slow_download, slow_upload) = crate::peer::test_pair(seed.info_hash()).await;
  let (good_download, good_upload) = crate::peer::test_pair(seed.info_hash()).await;
  let (requested, claimed) = tokio::sync::oneshot::channel();
  let slow_server = tokio::spawn(async move {
    slow_upload
      .control
      .send_text(
        &Control::Bitfield {
          bits: base64::engine::general_purpose::STANDARD.encode([0xe0]),
        }
        .encode()
        .unwrap(),
      )
      .await
      .unwrap();
    assert!(matches!(
      receive(&slow_upload).await,
      Control::Bitfield { .. }
    ));
    assert!(matches!(
      receive(&slow_upload).await,
      Control::Request { block_index: 0, .. }
    ));
    requested.send(()).unwrap();
    while let Some(event) = slow_upload.control.poll().await {
      if matches!(event, DataChannelEvent::OnClose) {
        break;
      }
    }
  });
  let slow = tokio::spawn({
    let seed = seed.clone();
    let queue = queue.clone();
    let store = store.clone();
    async move { slow_download.download_scheduled(&seed, store, queue).await }
  });
  queue.focus(0, 0, Duration::from_millis(150), true);
  claimed.await.unwrap();
  // Give the other RTC connection a distinct prefetch block.
  queue.focus(0, 1, Duration::from_millis(150), true);
  let good_server =
    tokio::spawn(async move { good_upload.serve_file(Arc::new(Mutex::new(prepared))).await });
  let good = tokio::spawn({
    let seed = seed.clone();
    let queue = queue.clone();
    let store = store.clone();
    async move { good_download.download_scheduled(&seed, store, queue).await }
  });
  let http = tokio::spawn(cdn(
    Arc::new(seed),
    store,
    queue.clone(),
    2,
    q2q_http::Executor::builder().build().unwrap(),
  ));
  tokio::time::timeout(Duration::from_secs(3), queue.verified(1))
    .await
    .unwrap()
    .unwrap();
  tokio::time::timeout(Duration::from_secs(3), queue.verified(0))
    .await
    .unwrap()
    .unwrap();
  assert!(matches!(slow.await.unwrap(), Err(TransferError::Timeout)));
  assert_eq!(server.requests.lock().unwrap().len(), 1);
  queue.focus(2, 0, Duration::from_secs(1), true);
  tokio::time::timeout(Duration::from_secs(3), queue.verified(2))
    .await
    .unwrap()
    .unwrap();
  good.await.unwrap().unwrap();
  good_server.await.unwrap().unwrap();
  slow_server.await.unwrap();
  http.await.unwrap();
  assert_eq!(server.requests.lock().unwrap().len(), 1);
  assert_eq!(queue.progress.borrow().verified_blocks, 3);
  assert_eq!(
    std::fs::read(dir.path().join("destination")).unwrap(),
    bytes
  );
}
