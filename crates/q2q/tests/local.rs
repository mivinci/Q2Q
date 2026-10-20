//! Opt-in real CLI processes; run against a fresh local Track/Punch pair.
use super::*;
use std::{
  fs::File,
  process::{Child, Stdio},
  time::{Duration, Instant},
};

struct Process(Child);
impl Process {
  fn wait(&mut self) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
      if let Some(status) = self.0.try_wait().unwrap() {
        return status;
      }
      assert!(
        Instant::now() < deadline,
        "CLI process did not finish in 60 seconds"
      );
      std::thread::sleep(Duration::from_millis(50));
    }
  }
  fn signal(&self, signal: &str) {
    assert!(
      Command::new("kill")
        .arg(signal)
        .arg(self.0.id().to_string())
        .status()
        .unwrap()
        .success()
    );
  }
}
impl Drop for Process {
  fn drop(&mut self) {
    let _ = self.0.kill();
    let _ = self.0.wait();
  }
}
fn wait_for(mut condition: impl FnMut() -> bool) {
  let deadline = Instant::now() + Duration::from_secs(45);
  while !condition() {
    assert!(Instant::now() < deadline, "condition not met in 45 seconds");
    std::thread::sleep(Duration::from_millis(50));
  }
}
fn logged(command: &mut Command, dir: &Path, name: &str) -> Process {
  command.stdout(Stdio::from(
    File::create(dir.join(format!("{name}.out"))).unwrap(),
  ));
  command.stderr(Stdio::from(
    File::create(dir.join(format!("{name}.err"))).unwrap(),
  ));
  Process(command.spawn().unwrap())
}

#[test]
#[ignore = "requires Q2Q_TEST_TRACK_URL pointing to a fresh local Track/Punch pair"]
fn publish_download_interrupt_resume_and_withdraw() {
  let track = std::env::var("Q2Q_TEST_TRACK_URL").expect("Q2Q_TEST_TRACK_URL");
  let dir = TempDir::new().unwrap();
  let source = dir.path().join("source");
  let bytes: Vec<_> = (0..4 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
  std::fs::write(&source, &bytes).unwrap();
  let seed_path = dir.path().join("content.q2q");
  let mut publish = cli(dir.path());
  publish
    .args(["--track", &track, "--upload-rate", "1048576", "publish"])
    .arg(&source)
    .arg("--seed")
    .arg(&seed_path);
  let mut publisher = logged(&mut publish, dir.path(), "publisher");
  wait_for(|| seed_path.exists());
  let mut seed = Seed::parse(&std::fs::read(&seed_path).unwrap()).unwrap();
  // A CDN entry must not hide initial Track failure and prevent trying the
  // reachable Track from the seed. Content identity is unchanged by sources.
  seed.cdn_list = vec!["http://127.0.0.1:1/unavailable-cdn".into()];
  std::fs::write(&seed_path, seed.encode()).unwrap();
  let destination = dir.path().join("received");
  let mut receive = cli(&dir.path().join("receiver"));
  receive
    .args(["--track", "http://127.0.0.1:1", "download"])
    .arg(&seed_path)
    .arg("-o")
    .arg(&destination);
  let mut receiver = logged(&mut receive, dir.path(), "receiver");
  let checkpoints = dir.path().join("receiver/state/checkpoints");
  wait_for(|| {
    std::fs::read_dir(&checkpoints).is_ok_and(|entries| {
      entries.flatten().any(|entry| {
        std::fs::read(entry.path())
          .is_ok_and(|data| data.iter().filter(|byte| **byte == b'\n').count() >= 2)
      })
    })
  });
  receiver.signal("-INT");
  assert_eq!(receiver.wait().code(), Some(130));
  // Reusing the same command must detect and resume the partial checkpoint
  // automatically, including retrying the unavailable preferred Track.
  let mut resumed = logged(&mut receive, dir.path(), "resumed");
  assert!(
    resumed.wait().success(),
    "{}",
    std::fs::read_to_string(dir.path().join("resumed.err")).unwrap()
  );
  assert_eq!(std::fs::read(&destination).unwrap(), bytes);
  assert_eq!(
    std::fs::read_to_string(dir.path().join("resumed.out")).unwrap(),
    format!("{}\n", destination.display())
  );
  let received_log = std::fs::read_to_string(dir.path().join("resumed.err")).unwrap();
  assert!(
    received_log.contains("trying candidate 2"),
    "missing Track failover: {received_log}"
  );
  assert!(
    received_log.contains("  peer "),
    "missing download Peer detail: {received_log}"
  );
  assert!(
    received_log.contains("received ") && received_log.contains("verified "),
    "missing byte counters: {received_log}"
  );
  publisher.signal("-TERM");
  assert_eq!(publisher.wait().code(), Some(130));
  let runtime = tokio::runtime::Runtime::new().unwrap();
  runtime.block_on(async {
    let client = q2q_core::client::builder()
      .track_url(&track)
      .identity_file(dir.path().join("inspector-identity"))
      .storage_dir(dir.path().join("inspector-state"))
      .build()
      .await
      .unwrap();
    let session = client.connect().await.unwrap();
    assert!(
      session
        .query(&seed.info_hash())
        .await
        .unwrap()
        .peers
        .is_empty(),
      "publication not withdrawn"
    );
  });
  // Full resume after publisher exit must complete without needing its network.
  let mut offline = logged(&mut receive, dir.path(), "offline");
  assert!(offline.wait().success());
}

#[test]
#[ignore = "requires Q2Q_TEST_TRACK_URL pointing to a fresh local Track/Punch pair"]
fn magnet_publish_download_interrupt_and_offline_resume() {
  let track = std::env::var("Q2Q_TEST_TRACK_URL").expect("Q2Q_TEST_TRACK_URL");
  let dir = TempDir::new().unwrap();
  let source = dir.path().join("magnet-photo.bin");
  let bytes: Vec<_> = (0..2 * 1024 * 1024 + 17).map(|i| (i % 251) as u8).collect();
  std::fs::write(&source, &bytes).unwrap();
  let seed_path = dir.path().join("photo.q2q");
  let mut command = cli(dir.path());
  command
    .args(["--track", &track, "--upload-rate", "262144", "publish"])
    .arg(&source)
    .arg("--seed")
    .arg(&seed_path);
  let mut publisher = logged(&mut command, dir.path(), "magnet-publisher");
  wait_for(|| seed_path.exists());
  let exported = cli(dir.path())
    .arg("magnet")
    .arg(&seed_path)
    .output()
    .unwrap();
  success(&exported);
  let link = String::from_utf8(exported.stdout)
    .unwrap()
    .trim()
    .to_owned();
  let receiver_dir = dir.path().join("receiver");
  std::fs::create_dir_all(&receiver_dir).unwrap();
  let output_path = receiver_dir.join("download.bin");
  let mut command = cli(&receiver_dir);
  command
    .args([
      "--quiet",
      "--track",
      "http://127.0.0.1:1",
      "download",
      &link,
    ])
    .arg("-o")
    .arg(&output_path);
  let mut receiver = logged(&mut command, dir.path(), "magnet-receiver");
  wait_for(|| {
    std::fs::read_dir(receiver_dir.join("state/checkpoints")).is_ok_and(|entries| {
      entries.filter_map(Result::ok).any(|entry| {
        std::fs::read_to_string(entry.path()).is_ok_and(|text| text.lines().count() >= 2)
      })
    })
  });
  receiver.signal("-INT");
  assert_eq!(receiver.wait().code(), Some(130));
  assert_eq!(
    std::fs::read_dir(receiver_dir.join("state/metadata"))
      .unwrap()
      .count(),
    1
  );
  let without_track = link.split("&tr=").next().unwrap();
  let result = cli(&receiver_dir)
    .args(["--quiet", "--track", &track, "download", without_track])
    .arg("-o")
    .arg(&output_path)
    .output()
    .unwrap();
  success(&result);
  assert_eq!(std::fs::read(&output_path).unwrap(), bytes);
  publisher.signal("-TERM");
  assert_eq!(publisher.wait().code(), Some(130));
  std::fs::write(receiver_dir.join("state/identity"), b"invalid identity").unwrap();
  let result = cli(&receiver_dir)
    .args([
      "--quiet",
      "--track",
      "http://127.0.0.1:1",
      "download",
      without_track,
    ])
    .arg("-o")
    .arg(&output_path)
    .output()
    .unwrap();
  success(&result);
  assert_eq!(
    String::from_utf8_lossy(&result.stdout).trim(),
    output_path.display().to_string()
  );
}

#[test]
fn interrupt_during_magnet_discovery_creates_no_destination_or_checkpoint() {
  use std::io::Read;
  let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
  let track = format!("http://{}", listener.local_addr().unwrap());
  let (ready, connected) = std::sync::mpsc::channel();
  let server = std::thread::spawn(move || {
    let (mut stream, _) = listener.accept().unwrap();
    stream
      .set_read_timeout(Some(Duration::from_secs(5)))
      .unwrap();
    ready.send(()).unwrap();
    let mut buffer = [0; 4096];
    while matches!(stream.read(&mut buffer), Ok(n) if n != 0) {}
  });
  let dir = TempDir::new().unwrap();
  let destination = dir.path().join("not-created.bin");
  let link = format!("magnet:?xt=urn:sha256:{}", "a".repeat(64));
  let mut command = cli(dir.path());
  command
    .args(["--quiet", "--track", &track, "download", &link])
    .arg("--output")
    .arg(&destination);
  let mut receiver = logged(&mut command, dir.path(), "metadata-cancel");
  connected.recv_timeout(Duration::from_secs(5)).unwrap();
  let interrupted = Instant::now();
  receiver.signal("-INT");
  assert_eq!(receiver.wait().code(), Some(130));
  assert!(interrupted.elapsed() < Duration::from_secs(3));
  assert!(!destination.exists());
  assert!(!dir.path().join("state/checkpoints").exists());
  assert!(!dir.path().join("state/metadata").exists());
  server.join().unwrap();
}

#[test]
#[ignore = "requires Q2Q_TEST_TRACK_URL pointing to a fresh local Track/Punch pair"]
fn seed_only_then_publish_seed_with_default_and_explicit_sources() {
  let track = std::env::var("Q2Q_TEST_TRACK_URL").expect("Q2Q_TEST_TRACK_URL");
  let dir = TempDir::new().unwrap();
  let source = dir.path().join("content.bin");
  let renamed = dir.path().join("renamed.bin");
  let bytes: Vec<_> = (0..800_001).map(|i| (i % 251) as u8).collect();
  std::fs::write(&source, &bytes).unwrap();
  let seed_path = dir.path().join("content.bin.q2q");
  success(
    &cli(dir.path())
      .args(["--track", &track, "--quiet", "seed"])
      .arg(&source)
      .output()
      .unwrap(),
  );
  assert!(!dir.path().join("state").exists());
  let original = std::fs::read(&seed_path).unwrap();
  for explicit in [false, true] {
    let export = dir.path().join("export.q2q");
    let mut command = cli(dir.path());
    command.args(["--quiet", "publish"]).arg(&seed_path);
    if explicit {
      command
        .arg("--file")
        .arg(&renamed)
        .arg("--seed")
        .arg(&export);
    }
    let name = if explicit {
      "explicit-seed"
    } else {
      "default-seed"
    };
    let mut publisher = logged(&mut command, dir.path(), name);
    wait_for(|| {
      std::fs::read_to_string(dir.path().join(format!("{name}.out")))
        .is_ok_and(|text| !text.is_empty())
    });
    let input = if explicit { &export } else { &seed_path };
    assert_eq!(
      Seed::parse(&std::fs::read(input).unwrap())
        .unwrap()
        .info_hash(),
      Seed::parse(&original).unwrap().info_hash()
    );
    let destination = dir.path().join(format!("download-{name}"));
    let result = cli(&dir.path().join(name))
      .args(["--quiet", "download"])
      .arg(input)
      .arg("-o")
      .arg(&destination)
      .output()
      .unwrap();
    success(&result);
    assert_eq!(std::fs::read(destination).unwrap(), bytes);
    assert_eq!(std::fs::read(&seed_path).unwrap(), original);
    publisher.signal("-TERM");
    assert_eq!(publisher.wait().code(), Some(130));
    if !explicit {
      std::fs::rename(&source, &renamed).unwrap();
    }
  }
}
