mod support;
use q2q_http::{Checksum, DownloadRequest, DownloadState, Downloader, Error};
use std::{
  sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
  },
  time::Duration,
};
use support::*;

#[tokio::test]
async fn small_files_checksums_empty_files_and_offline_completion() {
  let dir = tempfile::tempdir().unwrap();
  let server = Server::start(|_| Reply::body(b"hello")).await;
  let downloader = Downloader::builder().build().unwrap();
  let path = dir.path().join("sha1");
  let req = DownloadRequest::new(&path)
    .mirrors([&server.url])
    .expected_size(5)
    .checksum(Checksum::sha1("aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d").unwrap());
  let task = downloader.download(req.clone()).await.unwrap();
  let updates = task.subscribe_progress();
  task.wait().await.unwrap();
  assert_eq!(updates.borrow().state, DownloadState::Completed);
  assert_eq!(std::fs::read(&path).unwrap(), b"hello");
  downloader.resume(req).await.unwrap().wait().await.unwrap();
  assert_eq!(server.requests.lock().unwrap().len(), 1);
  let empty = DownloadRequest::new(dir.path().join("empty"))
    .expected_size(0)
    .checksum(sha256(b""));
  downloader
    .download(empty)
    .await
    .unwrap()
    .wait()
    .await
    .unwrap();
  assert!(Checksum::sha1("非ascii").is_err());
}
#[tokio::test]
async fn corrupt_mirror_is_not_published_and_next_mirror_succeeds() {
  let dir = tempfile::tempdir().unwrap();
  let bad = Server::start(|_| Reply::body(b"wrong")).await;
  let good = Server::start(|_| Reply::body(b"hello")).await;
  let path = dir.path().join("file");
  let req = request(&path, &bad.url, b"hello").mirrors([&bad.url, &good.url]);
  Downloader::builder()
    .retries(0)
    .build()
    .unwrap()
    .download(req)
    .await
    .unwrap()
    .wait()
    .await
    .unwrap();
  assert_eq!(std::fs::read(path).unwrap(), b"hello");
  let path = dir.path().join("bad");
  let req = request(&path, &bad.url, b"hello");
  assert!(matches!(
    Downloader::builder()
      .retries(0)
      .build()
      .unwrap()
      .download(req)
      .await
      .unwrap()
      .wait()
      .await,
    Err(Error::Integrity)
  ));
  assert!(!path.exists());
}
#[tokio::test]
async fn parallel_ranges_and_ignored_range_fallback() {
  let bytes = vec![17; 10 * 1024 + 7];
  let served = bytes.clone();
  let server = Server::start(move |r| Reply::range(r, &served).header("ETag", "\"one\"")).await;
  let dir = tempfile::tempdir().unwrap();
  let downloader = Downloader::builder()
    .range_threshold(1)
    .chunk_size(1024)
    .max_requests(3)
    .max_requests_per_host(2)
    .build()
    .unwrap();
  let path = dir.path().join("ranges");
  downloader
    .download(request(&path, &server.url, &bytes))
    .await
    .unwrap()
    .wait()
    .await
    .unwrap();
  assert_eq!(std::fs::read(path).unwrap(), bytes);
  assert!(
    server
      .requests
      .lock()
      .unwrap()
      .iter()
      .all(|r| r.range.is_some())
  );
  let served = bytes.clone();
  let server = Server::start(move |_| Reply::body(served.clone())).await;
  let path = dir.path().join("full");
  downloader
    .download(request(&path, &server.url, &bytes))
    .await
    .unwrap()
    .wait()
    .await
    .unwrap();
  assert_eq!(std::fs::read(path).unwrap(), bytes);
  let r = server.requests.lock().unwrap();
  assert!(r[0].range.is_some());
  assert!(r[1].range.is_none());
}
#[tokio::test]
async fn cancel_drains_and_resume_preserves_only_matching_strong_etag_ranges() {
  let bytes = vec![3; 8192];
  let served = bytes.clone();
  let slow = Arc::new(AtomicBool::new(true));
  let flag = slow.clone();
  let server = Server::start(move |r| {
    let mut reply = Reply::range(r, &served).header("ETag", "\"same\"");
    if flag.load(Ordering::SeqCst) && r.range.as_deref() != Some("bytes=0-1023") {
      reply.delay = Duration::from_millis(200);
    }
    reply
  })
  .await;
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("resume");
  let req = request(&path, &server.url, &bytes);
  let downloader = Downloader::builder()
    .range_threshold(1)
    .chunk_size(1024)
    .build()
    .unwrap();
  let task = downloader.download(req.clone()).await.unwrap();
  staged(&task, 1024).await;
  task.cancel();
  assert!(matches!(task.wait().await, Err(Error::Cancelled)));
  assert!(!path.exists());
  slow.store(false, Ordering::SeqCst);
  downloader.resume(req).await.unwrap().wait().await.unwrap();
  assert_eq!(std::fs::read(path).unwrap(), bytes);
  assert_eq!(
    server
      .requests
      .lock()
      .unwrap()
      .iter()
      .filter(|r| r.range.as_deref() == Some("bytes=0-1023"))
      .count(),
    1
  );
  assert!(
    server
      .requests
      .lock()
      .unwrap()
      .iter()
      .any(|r| r.if_range.as_deref() == Some("\"same\""))
  );
}
#[tokio::test]
async fn resumed_corruption_is_detected_and_repaired_without_publishing_bad_data() {
  use std::io::{Seek, SeekFrom, Write};
  let bytes = vec![4; 8192];
  let served = bytes.clone();
  let slow = Arc::new(AtomicBool::new(true));
  let flag = slow.clone();
  let server = Server::start(move |r| {
    let mut reply = Reply::range(r, &served).header("ETag", "\"same\"");
    if flag.load(Ordering::SeqCst) && r.range.as_deref() != Some("bytes=0-1023") {
      reply.delay = Duration::from_millis(200);
    }
    reply
  })
  .await;
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("corrupt");
  let req = request(&path, &server.url, &bytes);
  let downloader = Downloader::builder()
    .range_threshold(1)
    .chunk_size(1024)
    .build()
    .unwrap();
  let task = downloader.download(req.clone()).await.unwrap();
  staged(&task, 1024).await;
  task.cancel();
  let _ = task.wait().await;
  let mut part = std::fs::OpenOptions::new()
    .write(true)
    .open(dir.path().join("corrupt.q2q-http.part"))
    .unwrap();
  part.seek(SeekFrom::Start(0)).unwrap();
  part.write_all(b"bad").unwrap();
  drop(part);
  slow.store(false, Ordering::SeqCst);
  downloader.resume(req).await.unwrap().wait().await.unwrap();
  assert_eq!(std::fs::read(path).unwrap(), bytes);
}
#[tokio::test]
async fn retry_limits_no_clobber_and_destination_lock() {
  let count = Arc::new(AtomicUsize::new(0));
  let c = count.clone();
  let server = Server::start(move |_| {
    if c.fetch_add(1, Ordering::SeqCst) == 0 {
      let mut r = Reply::body(b"").header("Retry-After", "0");
      r.status = 429;
      r
    } else {
      Reply::body(b"ok")
    }
  })
  .await;
  let dir = tempfile::tempdir().unwrap();
  let downloader = Downloader::builder()
    .retry_delay(Duration::ZERO)
    .build()
    .unwrap();
  let path = dir.path().join("retry");
  downloader
    .download(request(&path, &server.url, b"ok"))
    .await
    .unwrap()
    .wait()
    .await
    .unwrap();
  assert_eq!(count.load(Ordering::SeqCst), 2);
  assert!(
    downloader
      .download(request(&path, &server.url, b"ok"))
      .await
      .unwrap()
      .wait()
      .await
      .is_err()
  );
  let server = Server::start(|_| {
    let mut r = Reply::body(b"ok");
    r.delay = Duration::from_millis(200);
    r
  })
  .await;
  let path = dir.path().join("locked");
  let req = request(&path, &server.url, b"ok");
  let task = downloader.download(req.clone()).await.unwrap();
  tokio::time::timeout(Duration::from_secs(2), async {
    while server.requests.lock().unwrap().is_empty() {
      tokio::task::yield_now().await;
    }
  })
  .await
  .unwrap();
  assert!(matches!(
    downloader.download(req.clone()).await.unwrap().wait().await,
    Err(Error::Busy)
  ));
  std::fs::write(&path, b"external").unwrap();
  assert!(task.wait().await.is_err());
  assert_eq!(std::fs::read(&path).unwrap(), b"external");
}
#[tokio::test]
async fn cancelled_admission_does_not_spawn_and_file_tasks_share_request_budget() {
  let server = Server::start(|_| {
    let mut r = Reply::body(b"ok");
    r.delay = Duration::from_millis(80);
    r
  })
  .await;
  let dir = tempfile::tempdir().unwrap();
  let downloader = Downloader::builder().max_active_files(1).build().unwrap();
  let first = downloader
    .download(request(dir.path().join("first"), &server.url, b"ok"))
    .await
    .unwrap();
  assert!(
    tokio::time::timeout(
      Duration::from_millis(10),
      downloader.download(request(dir.path().join("never"), &server.url, b"ok"))
    )
    .await
    .is_err()
  );
  first.wait().await.unwrap();
  assert!(!dir.path().join("never.q2q-http.part").exists());
  let downloader = Downloader::builder()
    .max_requests(1)
    .max_active_files(4)
    .build()
    .unwrap();
  let mut tasks = vec![];
  for i in 0..4 {
    tasks.push(
      downloader
        .download(request(
          dir.path().join(format!("file{i}")),
          &server.url,
          b"ok",
        ))
        .await
        .unwrap(),
    );
  }
  for task in tasks {
    task.wait().await.unwrap();
  }
  assert_eq!(server.peak.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn changed_validator_restarts_without_mixing_source_versions() {
  let changed = Arc::new(AtomicBool::new(false));
  let flag = changed.clone();
  let server = Server::start(move |r| {
    let new = flag.load(Ordering::SeqCst);
    let bytes = vec![if new { 8 } else { 7 }; 8192];
    let etag = if new { "\"new\"" } else { "\"old\"" };
    if r.if_range.as_deref().is_some_and(|v| v != etag) {
      return Reply::body(bytes).header("ETag", etag);
    }
    let mut reply = Reply::range(r, &bytes).header("ETag", etag);
    if !new && r.range.as_deref() != Some("bytes=0-1023") {
      reply.delay = Duration::from_millis(200);
    }
    reply
  })
  .await;
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("changed");
  let req = request(&path, &server.url, &vec![8; 8192]);
  let downloader = Downloader::builder()
    .range_threshold(1)
    .chunk_size(1024)
    .build()
    .unwrap();
  let task = downloader.download(req.clone()).await.unwrap();
  staged(&task, 1024).await;
  task.cancel();
  let _ = task.wait().await;
  changed.store(true, Ordering::SeqCst);
  downloader.resume(req).await.unwrap().wait().await.unwrap();
  assert_eq!(std::fs::read(&path).unwrap(), vec![8; 8192]);
  assert!(
    server
      .requests
      .lock()
      .unwrap()
      .iter()
      .any(|r| r.range.is_none())
  );
}

#[tokio::test]
async fn invalid_checkpoint_does_not_touch_staging_or_existing_target() {
  let server = Server::start(|r| {
    let mut reply = Reply::range(r, &vec![1; 8192]).header("ETag", "\"v\"");
    if r.range.as_deref() != Some("bytes=0-1023") {
      reply.delay = Duration::from_millis(200);
    }
    reply
  })
  .await;
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("journal");
  let req = request(&path, &server.url, &vec![1; 8192]);
  let downloader = Downloader::builder()
    .range_threshold(1)
    .chunk_size(1024)
    .build()
    .unwrap();
  let task = downloader.download(req.clone()).await.unwrap();
  staged(&task, 1024).await;
  task.cancel();
  let _ = task.wait().await;
  let checkpoint = dir.path().join("journal.q2q-http.resume");
  let part = dir.path().join("journal.q2q-http.part");
  let original = std::fs::read(&part).unwrap();
  let mut record: serde_json::Value =
    serde_json::from_slice(&std::fs::read(&checkpoint).unwrap()).unwrap();
  record["length"] = serde_json::json!(123);
  std::fs::write(&checkpoint, serde_json::to_vec(&record).unwrap()).unwrap();
  assert!(matches!(
    downloader.resume(req.clone()).await.unwrap().wait().await,
    Err(Error::Checkpoint(_))
  ));
  assert_eq!(std::fs::read(&part).unwrap(), original);
  std::fs::write(&path, vec![1; 8192]).unwrap();
  assert!(matches!(
    downloader.resume(req).await.unwrap().wait().await,
    Err(Error::Checkpoint(_))
  ));
  assert!(part.exists());
  assert!(checkpoint.exists());
}

#[tokio::test]
async fn dropping_wait_cancels_owner_and_eventually_unlocks_for_resume() {
  let slow = Arc::new(AtomicBool::new(true));
  let flag = slow.clone();
  let server = Server::start(move |_| {
    let mut reply = Reply::body(b"ok");
    if flag.load(Ordering::SeqCst) {
      reply.delay = Duration::from_secs(1);
    }
    reply
  })
  .await;
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("drop");
  let req = request(&path, &server.url, b"ok");
  let downloader = Downloader::builder().build().unwrap();
  let task = downloader.download(req.clone()).await.unwrap();
  let mut progress = task.subscribe_progress();
  assert!(
    tokio::time::timeout(Duration::from_millis(50), task.wait())
      .await
      .is_err()
  );
  tokio::time::timeout(Duration::from_secs(2), async {
    loop {
      let p = *progress.borrow_and_update();
      if p.state.is_terminal() {
        assert_eq!(p.state, DownloadState::Cancelled);
        break;
      }
      progress.changed().await.unwrap();
    }
  })
  .await
  .unwrap();
  slow.store(false, Ordering::SeqCst);
  downloader.resume(req).await.unwrap().wait().await.unwrap();
  assert_eq!(std::fs::read(&path).unwrap(), b"ok");
}

#[tokio::test]
async fn batch_of_small_and_large_artifacts_uses_one_bounded_downloader() {
  let small = vec![2; 1024];
  let large = vec![3; 1024 * 1024];
  let s = small.clone();
  let l = large.clone();
  let server = Server::start(move |r| {
    Reply::range(r, if r.path == "/small" { &s } else { &l }).header("ETag", "\"fixture\"")
  })
  .await;
  let dir = tempfile::tempdir().unwrap();
  let downloader = Downloader::builder()
    .max_active_files(4)
    .max_requests(4)
    .max_requests_per_host(4)
    .max_ranges_per_file(2)
    .range_threshold(8192)
    .chunk_size(256 * 1024)
    .build()
    .unwrap();
  let mut waiting: tokio::task::JoinSet<Result<std::path::PathBuf, Error>> =
    tokio::task::JoinSet::new();
  let start = std::time::Instant::now();
  for i in 0..132 {
    if waiting.len() == 4 {
      waiting.join_next().await.unwrap().unwrap().unwrap();
    }
    let (bytes, route) = if i < 128 {
      (&small, "small")
    } else {
      (&large, "large")
    };
    let task = downloader
      .download(request(
        dir.path().join(format!("artifact-{i}")),
        &format!("{}/{route}", server.url),
        bytes,
      ))
      .await
      .unwrap();
    waiting.spawn(task.wait());
  }
  while let Some(result) = waiting.join_next().await {
    result.unwrap().unwrap();
  }
  for i in 0..132 {
    assert_eq!(
      std::fs::read(dir.path().join(format!("artifact-{i}"))).unwrap(),
      if i < 128 {
        small.clone()
      } else {
        large.clone()
      }
    );
  }
  assert!(server.peak.load(Ordering::SeqCst) <= 4);
  eprintln!(
    "132 artifacts, {} bytes, 4 active files / 4 HTTP requests, {:?}",
    128 * small.len() + 4 * large.len(),
    start.elapsed()
  );
}

#[test]
fn crash_child() {
  let Ok(url) = std::env::var("Q2Q_HTTP_CRASH_URL") else {
    return;
  };
  let path = std::env::var_os("Q2Q_HTTP_CRASH_PATH").unwrap();
  tokio::runtime::Builder::new_current_thread()
    .enable_all()
    .build()
    .unwrap()
    .block_on(async {
      let downloader = Downloader::builder()
        .range_threshold(1)
        .chunk_size(1024)
        .build()
        .unwrap();
      let task = downloader
        .download(request(
          std::path::PathBuf::from(path),
          &url,
          &vec![9; 8192],
        ))
        .await
        .unwrap();
      staged(&task, 1024).await;
      // Abrupt exit deliberately skips Task/Store destructors and closes OS locks.
      std::process::exit(0);
    });
}

#[tokio::test]
async fn process_crash_releases_lock_and_preserves_durable_ranges() {
  let slow = Arc::new(AtomicBool::new(true));
  let flag = slow.clone();
  let server = Server::start(move |r| {
    let mut reply = Reply::range(r, &vec![9; 8192]).header("ETag", "\"stable\"");
    if flag.load(Ordering::SeqCst) && r.range.as_deref() != Some("bytes=0-1023") {
      reply.delay = Duration::from_millis(200);
    }
    reply
  })
  .await;
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("crash");
  let mut child = std::process::Command::new(std::env::current_exe().unwrap())
    .args(["--exact", "crash_child"])
    .env("Q2Q_HTTP_CRASH_URL", &server.url)
    .env("Q2Q_HTTP_CRASH_PATH", &path)
    .stdout(std::process::Stdio::null())
    .spawn()
    .unwrap();
  let exit = tokio::time::timeout(Duration::from_secs(10), async {
    loop {
      if let Some(status) = child.try_wait().unwrap() {
        break status;
      }
      tokio::time::sleep(Duration::from_millis(10)).await;
    }
  })
  .await;
  if exit.is_err() {
    let _ = child.kill();
    let _ = child.wait();
    panic!("crash child timed out");
  }
  assert!(exit.unwrap().success());
  assert!(!path.exists());
  slow.store(false, Ordering::SeqCst);
  Downloader::builder()
    .range_threshold(1)
    .chunk_size(1024)
    .build()
    .unwrap()
    .resume(request(&path, &server.url, &vec![9; 8192]))
    .await
    .unwrap()
    .wait()
    .await
    .unwrap();
  assert_eq!(std::fs::read(&path).unwrap(), vec![9; 8192]);
  assert_eq!(
    server
      .requests
      .lock()
      .unwrap()
      .iter()
      .filter(|r| r.range.as_deref() == Some("bytes=0-1023"))
      .count(),
    1
  );
}

#[tokio::test]
async fn journal_failure_stops_later_queued_writes_and_wait_releases_lock() {
  let server = Server::start(|r| {
    let mut reply = Reply::range(r, &vec![9; 8192]).header("ETag", "\"v\"");
    if r.range.as_deref() != Some("bytes=0-1023") {
      reply.delay = Duration::from_millis(200);
    }
    reply
  })
  .await;
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("disk-failure");
  let downloader = Downloader::builder()
    .range_threshold(1)
    .chunk_size(1024)
    .build()
    .unwrap();
  let task = downloader
    .download(request(&path, &server.url, &vec![9; 8192]))
    .await
    .unwrap();
  staged(&task, 1024).await;
  let journal = dir.path().join("disk-failure.q2q-http.resume");
  std::fs::rename(&journal, dir.path().join("saved-journal")).unwrap();
  std::fs::create_dir(&journal).unwrap();
  assert!(matches!(task.wait().await, Err(Error::Io(_))));
  assert!(!path.exists());
  let bytes = std::fs::read(dir.path().join("disk-failure.q2q-http.part")).unwrap();
  // The first failed journal commit may have written one more range, but the
  // poisoned store must reject all later queued writes before they touch disk.
  assert_eq!(bytes.iter().filter(|b| **b != 0).count(), 2048);
  let lock = std::fs::OpenOptions::new()
    .read(true)
    .write(true)
    .open(dir.path().join("disk-failure.q2q-http.lock"))
    .unwrap();
  lock.try_lock().unwrap();
}
