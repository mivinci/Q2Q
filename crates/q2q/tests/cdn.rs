use super::*;
use crate::block_queue::WorkQueue;
use crate::download_store::DownloadStore;
use crate::test_destination::destination_complete;
use std::sync::Mutex;

use crate::test_http::*;

#[tokio::test]
async fn grant_redirect_preserves_range_without_leaking_referer() {
  let bytes = vec![42; 262144 + 17];
  let mut seed = fixture(&bytes);
  let served = bytes.clone();
  let object = Server::start(move |request| range_reply(&request, &served)).await;
  let target = format!("{}/signed-object", object.url);
  let entry = Server::start(move |_| {
    let mut reply = Reply::body(Vec::new());
    reply.status = 307;
    reply.location = Some(target.clone());
    reply
  })
  .await;
  seed.cdn_list = vec![format!("{}/content?secret=fixture-only", entry.url)];
  let progress = crate::progress::channel(&seed, &[]);
  let source = Source::load(&seed, &progress).await.unwrap();
  for index in 0..2 {
    let (received, _) = source.fetch(&seed, index, &progress).await.unwrap();
    let start = index * 262144;
    assert_eq!(received, bytes[start..bytes.len().min(start + 262144)]);
  }
  let requests = object.requests.lock().unwrap();
  assert_eq!(requests.len(), 2);
  assert_eq!(requests[0].range.as_deref(), Some("bytes=0-262143"));
  assert_eq!(requests[1].range.as_deref(), Some("bytes=262144-262160"));
  assert!(
    requests
      .iter()
      .all(|request| request.referer.is_none() && request.identity)
  );
  // Each block returns to the stable entry; a signed redirect is never cached as its source.
  assert_eq!(entry.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn expired_signed_target_is_replaced_via_stable_entry_for_the_next_block() {
  use std::sync::atomic::{AtomicUsize, Ordering};
  let bytes = vec![7; 262144 + 17];
  let mut seed = fixture(&bytes);
  let generation = Arc::new(AtomicUsize::new(0));
  let validity = generation.clone();
  let served = bytes.clone();
  let object = Server::start(move |request| {
    if request.path != format!("/signed-{}", validity.load(Ordering::SeqCst)) {
      let mut denied = Reply::body(Vec::new());
      denied.status = 403;
      return denied;
    }
    range_reply(&request, &served)
  })
  .await;
  let signing = generation.clone();
  let target = object.url.clone();
  let entry = Server::start(move |_| {
    let mut reply = Reply::body(Vec::new());
    reply.status = 307;
    reply.location = Some(format!(
      "{target}/signed-{}",
      signing.load(Ordering::SeqCst)
    ));
    reply
  })
  .await;
  seed.cdn_list = vec![format!("{}/content?secret=fixture-only", entry.url)];
  let progress = crate::progress::channel(&seed, &[]);
  let source = Source::load(&seed, &progress).await.unwrap();
  source.fetch(&seed, 0, &progress).await.unwrap();
  generation.store(1, Ordering::SeqCst);
  let expired = Client::new()
    .get(format!("{}/signed-0", object.url))
    .send()
    .await
    .unwrap();
  assert_eq!(expired.status(), StatusCode::FORBIDDEN);
  let (tail, _) = source.fetch(&seed, 1, &progress).await.unwrap();
  assert_eq!(tail, bytes[262144..]);
  assert_eq!(entry.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn invalid_range_status_encoding_and_body_lengths_are_rejected() {
  let bytes = vec![9; 262144 + 5];
  let seed = fixture(&bytes);
  let server = Server::start(move |request| {
    let mut reply = range_reply(&request, &bytes);
    match request.path.as_str() {
      "/wrong-range" => reply.range = Some("bytes 1-262144/262149".into()),
      "/missing-range" => reply.range = None,
      "/full-response" => {
        reply.status = 200;
        reply.range = None;
      }
      "/status" => reply.status = 404,
      "/compressed" => reply.encoding = Some("gzip"),
      "/short" => {
        reply.body.pop();
        reply.length = None;
      }
      "/long" => {
        reply.body.push(0);
        reply.length = None;
      }
      "/bad-length" => reply.length = Some(1),
      "/no-length" => reply.length = None,
      _ => panic!(),
    }
    reply
  })
  .await;
  let http = Client::builder()
    .timeout(Duration::from_secs(2))
    .build()
    .unwrap();
  let progress = crate::progress::channel(&seed, &[]);
  for route in [
    "wrong-range",
    "missing-range",
    "full-response",
    "status",
    "compressed",
    "short",
    "long",
    "bad-length",
  ] {
    assert!(
      matches!(
        block(
          &http,
          Url::parse(&format!("{}/{route}", server.url)).unwrap(),
          &seed,
          0,
          &progress
        )
        .await,
        Err(TransferError::Cdn(_))
      ),
      "{route}"
    );
  }
  assert_eq!(
    block(
      &http,
      Url::parse(&format!("{}/no-length", server.url)).unwrap(),
      &seed,
      0,
      &progress
    )
    .await
    .unwrap()
    .len(),
    262144
  );
  assert_eq!(progress.borrow().active_cdn_requests, 0);
}

#[tokio::test]
async fn cdn_fills_only_missing_blocks_after_a_real_peer_fails() {
  let dir = tempfile::tempdir().unwrap();
  let bytes = vec![45; 262144 + 17];
  let source = dir.path().join("source");
  std::fs::write(&source, &bytes).unwrap();
  let prepared =
    crate::publication::PreparedFile::open(source.clone(), "http://localhost".into()).unwrap();
  let mut seed = prepared.seed().clone();
  let served = bytes.clone();
  let server = Server::start(move |request| range_reply(&request, &served)).await;
  seed.cdn_list = vec![format!("{}/data", server.url)];
  let path = dir.path().join("data");
  let mut store = DownloadStore::create(&seed, &path).unwrap();
  // This durable receipt is the prefix retained from a previous peer/session.
  store
    .commit(
      0,
      &bytes[..262144],
      prepared.proof(0).unwrap(),
      &seed.info.root,
    )
    .unwrap();
  drop(store);
  let restored = crate::test_destination::open_destination(&seed, &path, true)
    .await
    .unwrap();
  let progress = crate::progress::channel(&seed, &[0x80]);
  std::fs::write(source, vec![255; bytes.len()]).unwrap();
  let (download_peer, upload) = crate::peer::test_pair(seed.info_hash()).await;
  let (p2p, serving) = tokio::time::timeout(Duration::from_secs(10), async {
    tokio::join!(
      crate::scheduler::download(
        vec![download_peer],
        seed.clone(),
        restored.clone(),
        1,
        progress.clone(),
        |peer| async { Ok(peer) }
      ),
      upload.serve_file(Arc::new(Mutex::new(prepared))),
    )
  })
  .await
  .unwrap();
  assert!(p2p.unwrap_err().peer_retryable());
  assert!(matches!(
    serving,
    Err(TransferError::Prepare(
      crate::publication::PrepareError::SourceChanged
    ))
  ));
  assert_eq!(progress.borrow().verified_bytes, 262144);
  download(seed, restored.clone(), 2, progress.clone())
    .await
    .unwrap();
  assert_eq!(progress.borrow().verified_bytes, bytes.len() as u64);
  assert!(destination_complete(&restored).unwrap());
  assert_eq!(std::fs::read(path).unwrap(), bytes);
  let requests = server.requests.lock().unwrap();
  assert_eq!(requests.len(), 1);
  assert_eq!(requests[0].range.as_deref(), Some("bytes=262144-262160"));
}

#[tokio::test]
async fn fallback_waits_for_cancelled_workers_queued_disk_jobs() {
  let dir = tempfile::tempdir().unwrap();
  let bytes = b"durable block";
  let mut seed = fixture(bytes);
  let server = Server::start(|_| panic!("already committed block must not reach CDN")).await;
  seed.cdn_list = vec![format!("{}/unused", server.url)];
  let path = dir.path().join("data");
  let store = crate::test_destination::create_destination(&seed, &path)
    .await
    .unwrap();
  let progress = crate::progress::channel(&seed, &[]);
  let queue = WorkQueue::new(&seed, &[], progress.clone());
  let lease = queue.claim().await.unwrap();
  let scope = crate::scheduler::TaskScope::new();
  let worker_guard = scope.enter();
  let disk_guard = scope.enter();
  let proof = merkle::prove(seed.leaf_hashes.as_ref().unwrap(), 0);
  let root = seed.info.root;
  let destination = store.clone();
  let (release, blocked) = std::sync::mpsc::channel();
  let (ready, started) = tokio::sync::oneshot::channel();
  let worker = tokio::spawn(async move {
    let _worker = worker_guard;
    let disk = tokio::task::spawn_blocking(move || {
      let _disk = disk_guard;
      blocked.recv().unwrap();
      destination
        .lock()
        .unwrap()
        .commit(0, bytes, proof, &root)
        .unwrap();
      lease.commit();
    });
    ready.send(()).unwrap();
    disk.await.unwrap();
  });
  started.await.unwrap();
  worker.abort();
  assert!(worker.await.unwrap_err().is_cancelled());
  {
    let next_seed = seed.clone();
    let next_store = store.clone();
    let next_progress = progress.clone();
    let future = async move {
      scope.idle().await;
      download(next_seed, next_store, 2, next_progress).await
    };
    tokio::pin!(future);
    assert!(
      tokio::time::timeout(Duration::from_millis(10), future.as_mut())
        .await
        .is_err()
    );
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), future.as_mut())
      .await
      .unwrap()
      .unwrap();
  }
  assert_eq!(progress.borrow().verified_bytes, bytes.len() as u64);
  assert_eq!(progress.borrow().verified_blocks, 1);
  assert_eq!(server.requests.lock().unwrap().len(), 0);
  // A duplicated receipt would fail a subsequent resume's uniqueness check.
  drop(store);
  tokio::time::timeout(Duration::from_secs(2), async {
    loop {
      match DownloadStore::resume(&seed, &path) {
        Ok(restored) => {
          assert_eq!(restored.verified_bitfield(), vec![0x80]);
          break;
        }
        Err(TransferError::Checkpoint(message)) if message.starts_with("destination is locked") => {
          tokio::task::yield_now().await
        }
        Err(error) => panic!("resume failed: {error}"),
      }
    }
  })
  .await
  .unwrap();
}
