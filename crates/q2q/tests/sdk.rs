//! Integration coverage using only the public q2q API.
use q2q::client::{self, TransferError, TransferState};
use std::sync::atomic::Ordering;
use std::time::Duration;

mod http;
use http::*;

#[tokio::test]
async fn public_cold_start_and_resume_use_exact_ranges_with_unavailable_track() {
  let bytes = vec![63; 2 * 262144 + 17];
  let served = bytes.clone();
  let server = Server::start(move |request| {
    assert!(request.path.starts_with("/data?token=test"));
    range_reply(&request, &served)
  })
  .await;
  let mut seed = fixture(&bytes);
  seed.cdn_list = vec![format!("{}/data?token=test", server.url)];
  seed.proof_list = vec![format!(
    "{}/must-not-fetch-inline-takes-precedence",
    server.url
  )];
  let dir = tempfile::tempdir().unwrap();
  let client = client(dir.path(), 2).await;
  let path = dir.path().join("download.bin");
  let transfer = client.download(&seed, &path).await.unwrap();
  let updates = transfer.subscribe_progress();
  transfer.wait().await.unwrap();
  assert_eq!(std::fs::read(&path).unwrap(), bytes);
  assert_eq!(updates.borrow().state, TransferState::Completed);
  assert_eq!(updates.borrow().verified_bytes, bytes.len() as u64);
  assert_eq!(updates.borrow().active_cdn_requests, 0);
  let mut ranges: Vec<_> = server
    .requests
    .lock()
    .unwrap()
    .iter()
    .map(|r| {
      assert!(r.identity);
      r.range.clone().unwrap()
    })
    .collect();
  ranges.sort();
  assert_eq!(
    ranges,
    [
      "bytes=0-262143",
      "bytes=262144-524287",
      "bytes=524288-524304"
    ]
  );
  use std::io::{Seek, SeekFrom, Write};
  let mut disk = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
  disk.seek(SeekFrom::Start(2)).unwrap();
  disk.write_all(&[255]).unwrap();
  drop(disk);
  let transfer = client.resume_download(&seed, &path).await.unwrap();
  assert_eq!(
    transfer.progress().verified_bytes,
    (bytes.len() - 262144) as u64
  );
  transfer.wait().await.unwrap();
  let requests = server.requests.lock().unwrap();
  assert_eq!(requests.len(), 4);
  assert_eq!(
    requests.last().unwrap().range.as_deref(),
    Some("bytes=0-262143")
  );
  drop(requests);
  assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[tokio::test]
async fn proof_and_data_mirrors_fail_over_without_redownloading_verified_blocks() {
  let bytes = vec![74; 2 * 262144 + 17];
  let mut seed = fixture(&bytes);
  let hashes: Vec<_> = seed
    .leaf_hashes
    .take()
    .unwrap()
    .into_iter()
    .flatten()
    .collect();
  let served = bytes.clone();
  let server = Server::start(move |request| match request.path.as_str() {
    "/bad-hashes" => Reply::body(vec![0; hashes.len()]),
    "/hashes" => Reply::body(hashes.clone()),
    "/bad-data" => {
      let mut reply = range_reply(&request, &served);
      if request.range.as_deref() != Some("bytes=0-262143") {
        reply.body[0] ^= 1;
      }
      reply
    }
    "/data" => range_reply(&request, &served),
    _ => panic!("unexpected path"),
  })
  .await;
  seed.proof_list = vec![
    format!("{}/bad-hashes", server.url),
    format!("{}/hashes", server.url),
  ];
  seed.cdn_list = vec![
    "ftp://unsupported/file".into(),
    format!("{}/bad-data", server.url),
    format!("{}/data", server.url),
  ];
  let dir = tempfile::tempdir().unwrap();
  let client = client(dir.path(), 1).await;
  let path = dir.path().join("download.bin");
  client
    .download(&seed, &path)
    .await
    .unwrap()
    .wait()
    .await
    .unwrap();
  assert_eq!(std::fs::read(&path).unwrap(), bytes);
  {
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.iter().filter(|r| r.path == "/hashes").count(), 1);
    assert_eq!(requests.iter().filter(|r| r.path == "/data").count(), 2);
    assert!(
      requests
        .iter()
        .filter(|r| r.path == "/data")
        .all(|r| r.range.as_deref() != Some("bytes=0-262143"))
    );
  }
  let resumed = client.resume_download(&seed, &path).await.unwrap();
  assert_eq!(resumed.progress().verified_bytes, bytes.len() as u64);
  resumed.wait().await.unwrap();
}

#[tokio::test]
async fn missing_or_invalid_proof_tables_never_fetch_content() {
  for mode in 0..4 {
    let bytes = vec![5; 262144 + 7];
    let mut seed = fixture(&bytes);
    seed.leaf_hashes = None;
    let server = Server::start(move |request| {
      assert_eq!(request.path, "/hashes");
      Reply::body(match mode {
        1 => vec![0; 64],
        2 => vec![0; 63],
        _ => vec![0; 65],
      })
    })
    .await;
    seed.cdn_list = vec![format!("{}/must-not-fetch", server.url)];
    if mode != 0 {
      seed.proof_list = vec![format!("{}/hashes", server.url)];
    }
    let dir = tempfile::tempdir().unwrap();
    let client = client(dir.path(), 1).await;
    let transfer = client
      .download(&seed, dir.path().join("data"))
      .await
      .unwrap();
    let updates = transfer.subscribe_progress();
    assert!(matches!(transfer.wait().await, Err(TransferError::Cdn(_))));
    assert_eq!(updates.borrow().verified_bytes, 0);
    assert_eq!(updates.borrow().state, TransferState::Failed);
    assert_eq!(
      server.requests.lock().unwrap().len(),
      usize::from(mode != 0)
    );
  }
}

#[tokio::test]
async fn single_block_200_is_verified_and_http_can_be_cancelled_mid_body() {
  let dir = tempfile::tempdir().unwrap();
  let mut seed = fixture(b"one block");
  let server = Server::start(|request| {
    if request.path == "/hang" {
      let mut reply = Reply::body(vec![]);
      reply.status = 206;
      reply.range = Some("bytes 0-8/9".into());
      reply.length = Some(9);
      reply.hang = true;
      reply
    } else {
      Reply::body(b"one block".to_vec())
    }
  })
  .await;
  let client = client(dir.path(), 1).await;
  seed.cdn_list = vec![format!("{}/data", server.url)];
  client
    .download(&seed, dir.path().join("complete"))
    .await
    .unwrap()
    .wait()
    .await
    .unwrap();
  seed.cdn_list = vec![format!("{}/hang", server.url)];
  let path = dir.path().join("cancelled");
  let transfer = client.download(&seed, &path).await.unwrap();
  let updates = transfer.subscribe_progress();
  tokio::time::timeout(Duration::from_secs(2), async {
    while server.hanging.load(Ordering::SeqCst) == 0 {
      tokio::task::yield_now().await;
    }
  })
  .await
  .unwrap();
  assert_eq!(transfer.progress().active_cdn_requests, 1);
  transfer.cancel();
  tokio::time::timeout(Duration::from_secs(2), async {
    while server.closed.load(Ordering::SeqCst) == 0 {
      tokio::task::yield_now().await;
    }
  })
  .await
  .unwrap();
  assert_eq!(updates.borrow().state, TransferState::Cancelled);
  assert_eq!(updates.borrow().verified_bytes, 0);
  assert_eq!(updates.borrow().active_cdn_requests, 0);
  let resumed = client.resume_download(&seed, &path).await.unwrap();
  assert_eq!(resumed.progress().verified_bytes, 0);
  resumed.cancel();
}

#[tokio::test]
async fn invalid_content_never_reaches_the_destination_or_verified_progress() {
  let dir = tempfile::tempdir().unwrap();
  let mut seed = fixture(b"original content");
  let server = Server::start(|_| Reply::body(b"corrupt! content".to_vec())).await;
  seed.cdn_list = vec![format!("{}/data", server.url)];
  let client = client(dir.path(), 1).await;
  let path = dir.path().join("data");
  let transfer = client.download(&seed, &path).await.unwrap();
  let updates = transfer.subscribe_progress();
  assert!(matches!(transfer.wait().await, Err(TransferError::Cdn(_))));
  assert_eq!(updates.borrow().verified_bytes, 0);
  assert!(std::fs::read(&path).unwrap().iter().all(|b| *b == 0));
  let resumed = client.resume_download(&seed, &path).await.unwrap();
  assert_eq!(resumed.progress().verified_blocks, 0);
  resumed.cancel();
}

#[tokio::test]
async fn empty_new_download_requires_neither_track_nor_http() {
  let dir = tempfile::tempdir().unwrap();
  let client = client(dir.path(), 2).await;
  let seed = fixture(b"");
  let path = dir.path().join("empty");
  let transfer = client.download(&seed, &path).await.unwrap();
  let updates = transfer.subscribe_progress();
  transfer.wait().await.unwrap();
  assert_eq!(std::fs::metadata(path).unwrap().len(), 0);
  assert_eq!(updates.borrow().state, TransferState::Completed);
  assert_eq!(updates.borrow().active_cdn_requests, 0);
}
