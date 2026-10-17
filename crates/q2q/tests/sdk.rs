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

#[tokio::test]
async fn seekable_open_fetches_only_window_then_resumes_offline() {
  use std::io::SeekFrom;
  use tokio::io::{AsyncReadExt, AsyncSeekExt};
  let bytes: Vec<_> = (0..4 * 262144 + 19).map(|i| (i % 251) as u8).collect();
  let served = bytes.clone();
  let server = Server::start(move |request| range_reply(&request, &served)).await;
  let mut seed = fixture(&bytes);
  seed.cdn_list = vec![server.url.clone()];
  let dir = tempfile::tempdir().unwrap();
  let client = client(dir.path(), 2).await;
  let path = dir.path().join("vod");
  let mut file = client.open(&seed, &path).prefetch_blocks(0).await.unwrap();
  assert_eq!(file.len(), bytes.len() as u64);
  assert!(!file.is_empty());
  assert_eq!(file.progress().verified_blocks, 0);
  file.seek(SeekFrom::End(0)).await.unwrap();
  assert_eq!(file.read(&mut [0; 1]).await.unwrap(), 0);
  tokio::task::yield_now().await;
  assert!(server.requests.lock().unwrap().is_empty());
  for (offset, count) in [
    (2 * 262144 + 31, 97),
    (262144 - 8, 31),
    (bytes.len() - 9, 20),
    (0, 41),
  ] {
    file.seek(SeekFrom::Start(offset as u64)).await.unwrap();
    let mut output = vec![0; count.min(bytes.len() - offset)];
    file.read_exact(&mut output).await.unwrap();
    assert_eq!(output, bytes[offset..offset + output.len()]);
  }
  assert_eq!(file.position(), 41);
  assert!(file.seek(SeekFrom::Current(-42)).await.is_err());
  assert_eq!(file.position(), 41);
  file.seek(SeekFrom::End(99)).await.unwrap();
  assert_eq!(file.read(&mut [0; 1]).await.unwrap(), 0);
  file.seek(SeekFrom::Start(u64::MAX)).await.unwrap();
  assert!(file.seek(SeekFrom::Current(1)).await.is_err());
  assert_eq!(file.position(), u64::MAX);
  assert_eq!(server.requests.lock().unwrap().len(), 4);
  file.seek(SeekFrom::Start(0)).await.unwrap();
  let mut all = Vec::new();
  file.read_to_end(&mut all).await.unwrap();
  assert_eq!(all, bytes);
  let mut updates = file.subscribe_progress();
  tokio::time::timeout(Duration::from_secs(3), async {
    while updates.borrow_and_update().state == TransferState::Downloading {
      updates.changed().await.unwrap();
    }
  })
  .await
  .unwrap();
  assert_eq!(updates.borrow().state, TransferState::Completed);
  drop(file);
  drop(server);
  seed.cdn_list.clear();
  let mut file = client.open(&seed, &path).await.unwrap();
  assert_eq!(file.progress().verified_blocks, 5);
  let mut all = Vec::new();
  file.read_to_end(&mut all).await.unwrap();
  assert_eq!(all, bytes);
}

#[tokio::test]
async fn cancelled_read_seek_and_timeout_never_expose_sparse_bytes() {
  use std::io::SeekFrom;
  use tokio::io::{AsyncReadExt, AsyncSeekExt};
  let bytes = vec![87; 262144 + 17];
  let served = bytes.clone();
  let server = Server::start(move |request| {
    let mut reply = range_reply(&request, &served);
    if request.range.as_deref() == Some("bytes=0-262143") {
      reply.hang = true;
    }
    reply
  })
  .await;
  let mut seed = fixture(&bytes);
  seed.cdn_list = vec![server.url.clone()];
  let dir = tempfile::tempdir().unwrap();
  let client = client(dir.path(), 2).await;
  let path = dir.path().join("vod");
  let mut file = client
    .open(&seed, &path)
    .prefetch_blocks(0)
    // Include filesystem sync and competing tests on shared CI runners.
    .read_timeout(Duration::from_secs(2))
    .await
    .unwrap();
  let mut buffer = [9; 7];
  assert!(
    tokio::time::timeout(Duration::from_millis(20), file.read(&mut buffer))
      .await
      .is_err()
  );
  assert_eq!(file.position(), 0);
  assert_eq!(buffer, [9; 7]);
  file.seek(SeekFrom::End(-7)).await.unwrap();
  file.read_exact(&mut buffer).await.unwrap();
  assert_eq!(buffer, [87; 7]);
  file.seek(SeekFrom::Start(0)).await.unwrap();
  let error = file.read(&mut buffer).await.unwrap_err();
  assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
  assert_eq!(file.position(), 0);
  let updates = file.subscribe_progress();
  drop(file);
  assert_eq!(updates.borrow().state, TransferState::Cancelled);
  tokio::time::timeout(Duration::from_secs(2), async {
    while server.hanging.load(Ordering::SeqCst) != server.closed.load(Ordering::SeqCst) {
      tokio::task::yield_now().await;
    }
  })
  .await
  .unwrap();
  // Wait for the cancelled driver's store reference to be released.
  let mut file = tokio::time::timeout(Duration::from_secs(2), async {
    loop {
      match client.open(&seed, &path).prefetch_blocks(0).await {
        Ok(file) => break file,
        Err(_) => tokio::task::yield_now().await,
      }
    }
  })
  .await
  .unwrap();
  assert_eq!(file.progress().verified_blocks, 1);
  file.seek(SeekFrom::End(-7)).await.unwrap();
  file.read_exact(&mut buffer).await.unwrap();
  assert_eq!(buffer, [87; 7]);
}

#[tokio::test]
async fn vod_rejects_corrupt_http_and_changed_verified_disk() {
  use std::io::{Seek, SeekFrom, Write};
  use tokio::io::{AsyncReadExt, AsyncSeekExt};
  let bytes = vec![29; 31];
  let server = Server::start(|_| Reply::body(vec![99; 31])).await;
  let mut seed = fixture(&bytes);
  seed.cdn_list = vec![server.url.clone()];
  let dir = tempfile::tempdir().unwrap();
  let client = client(dir.path(), 1).await;
  let mut file = client.open(&seed, dir.path().join("bad")).await.unwrap();
  let mut buffer = [7; 31];
  assert!(file.read(&mut buffer).await.is_err());
  assert_eq!(buffer, [7; 31]);
  assert_eq!(file.progress().verified_blocks, 0);
  let served = bytes.clone();
  let server = Server::start(move |_| Reply::body(served.clone())).await;
  seed.cdn_list = vec![server.url.clone()];
  let path = dir.path().join("good");
  let mut file = client.open(&seed, &path).await.unwrap();
  file.read_exact(&mut buffer).await.unwrap();
  assert_eq!(buffer, [29; 31]);
  let mut disk = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
  disk.seek(SeekFrom::Start(0)).unwrap();
  disk.write_all(&[0]).unwrap();
  disk.sync_data().unwrap();
  file.seek(SeekFrom::Start(0)).await.unwrap();
  assert_eq!(
    file.read(&mut buffer).await.unwrap_err().kind(),
    std::io::ErrorKind::InvalidData
  );
}

#[tokio::test]
async fn empty_vod_and_invalid_options_need_no_network() {
  use tokio::io::{AsyncReadExt, AsyncSeekExt};
  let dir = tempfile::tempdir().unwrap();
  let client = client(dir.path(), 1).await;
  let seed = fixture(&[]);
  assert!(
    client
      .open(&seed, dir.path().join("bad"))
      .prefetch_blocks(65)
      .await
      .is_err()
  );
  assert!(
    client
      .open(&seed, dir.path().join("bad"))
      .read_timeout(Duration::ZERO)
      .await
      .is_err()
  );
  let mut file = client.open(&seed, dir.path().join("empty")).await.unwrap();
  assert!(file.is_empty());
  assert_eq!(file.read(&mut [1; 1]).await.unwrap(), 0);
  assert_eq!(file.seek(std::io::SeekFrom::End(0)).await.unwrap(), 0);
}
