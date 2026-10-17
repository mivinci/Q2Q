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
  let seed = Seed::parse(&std::fs::read(&seed_path).unwrap()).unwrap();
  let destination = dir.path().join("received");
  let mut receive = cli(&dir.path().join("receiver"));
  receive
    .arg("download")
    .arg(&seed_path)
    .arg("-o")
    .arg(&destination);
  let mut receiver = logged(&mut receive, dir.path(), "receiver");
  let receipt = dir.path().join("received.q2q.resume");
  wait_for(|| {
    std::fs::read(&receipt).is_ok_and(|data| data.iter().filter(|b| **b == b'\n').count() >= 2)
  });
  receiver.signal("-INT");
  assert_eq!(receiver.wait().code(), Some(130));
  // The same process command and identity resume independently of publication.
  receive.arg("--resume");
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
    let client = q2q::client::builder()
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
