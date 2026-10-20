use q2q_primitives::{
  merkle::EMPTY_ROOT,
  seed::{Info, Seed},
};
use std::{
  path::Path,
  process::{Command, Output},
};
use tempfile::TempDir;

fn cli(dir: &Path) -> Command {
  let mut command = Command::new(env!("CARGO_BIN_EXE_q2q"));
  for name in ["Q2Q_TRACK_URL", "Q2Q_HOME", "Q2Q_IDENTITY_FILE"] {
    command.env_remove(name);
  }
  command.env("Q2Q_HOME", dir.join("state"));
  command
}
fn empty_seed(dir: &Path) -> std::path::PathBuf {
  let seed = Seed::new(Info::new(0, EMPTY_ROOT));
  let path = dir.join("empty.q2q");
  std::fs::write(&path, seed.encode()).unwrap();
  path
}
fn success(output: &Output) {
  assert!(
    output.status.success(),
    "{}",
    String::from_utf8_lossy(&output.stderr)
  );
}

#[test]
fn help_and_version_succeed_without_creating_state() {
  let dir = TempDir::new().unwrap();
  for argument in ["--help", "--version"] {
    let output = cli(dir.path()).arg(argument).output().unwrap();
    success(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("q2q"));
    if argument == "--help" {
      assert!(String::from_utf8_lossy(&output.stdout).contains("--insecure"));
    }
  }
  assert!(!dir.path().join("state").exists());
}

#[test]
fn invalid_arguments_exit_two_without_creating_state() {
  let dir = TempDir::new().unwrap();
  for args in [
    vec!["download"],
    vec!["publish"],
    vec!["--upload-rate", "0", "publish", "file", "--seed", "seed"],
    vec!["--download-peers", "17", "download", "seed", "-o", "file"],
    vec!["--upload-peers", "0", "publish", "file", "--seed", "seed"],
  ] {
    let output = cli(dir.path()).args(args).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
  }
  assert!(!dir.path().join("state").exists());
}

#[test]
fn malformed_and_oversized_seeds_leave_destination_untouched() {
  let dir = TempDir::new().unwrap();
  let seed = dir.path().join("bad.q2q");
  let destination = dir.path().join("output");
  for bytes in [b"not a seed".to_vec(), vec![0; 8 * 1024 * 1024 + 1]] {
    std::fs::write(&seed, bytes).unwrap();
    let output = cli(dir.path())
      .arg("download")
      .arg(&seed)
      .arg("-o")
      .arg(&destination)
      .output()
      .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(!destination.exists());
  }
}

#[test]
fn seed_export_refuses_existing_file_before_network_access() {
  let dir = TempDir::new().unwrap();
  let seed = dir.path().join("existing.q2q");
  std::fs::write(&seed, b"keep this").unwrap();
  let output = cli(dir.path())
    .args(["--track", "http://127.0.0.1:1", "publish"])
    .arg(dir.path().join("missing-source"))
    .arg("--seed")
    .arg(&seed)
    .output()
    .unwrap();
  assert_eq!(output.status.code(), Some(1));
  assert!(String::from_utf8_lossy(&output.stderr).contains("seed output already exists"));
  assert_eq!(std::fs::read(seed).unwrap(), b"keep this");
}

#[test]
fn empty_download_and_offline_resume_preserve_stdout_contract() {
  let dir = TempDir::new().unwrap();
  let seed = empty_seed(dir.path());
  let destination = dir.path().join("output");
  let mut first_identity = Vec::new();
  for resume in [false, true] {
    let mut command = cli(dir.path());
    command
      .arg("download")
      .arg(&seed)
      .arg("--output")
      .arg(&destination)
      .arg("--quiet");
    if resume {
      command.arg("--resume");
    }
    let output = command.output().unwrap();
    success(&output);
    assert_eq!(
      String::from_utf8(output.stdout).unwrap(),
      format!("{}\n", destination.display())
    );
    assert!(output.stderr.is_empty());
    assert_eq!(std::fs::metadata(&destination).unwrap().len(), 0);
    assert!(dir.path().join("state/checkpoints").is_dir());
    assert_eq!(
      std::fs::read_dir(dir.path().join("state/checkpoints"))
        .unwrap()
        .count(),
      1
    );
    let identity = std::fs::read(dir.path().join("state/identity")).unwrap();
    if resume {
      assert_eq!(identity, first_identity);
    } else {
      first_identity = identity;
    }
  }
}

#[test]
fn normal_download_refuses_existing_target_and_missing_resume_is_an_error() {
  let dir = TempDir::new().unwrap();
  let seed = empty_seed(dir.path());
  let destination = dir.path().join("output");
  std::fs::write(&destination, b"original").unwrap();
  let output = cli(dir.path())
    .arg("download")
    .arg(&seed)
    .arg("-o")
    .arg(&destination)
    .output()
    .unwrap();
  assert_eq!(output.status.code(), Some(1));
  assert_eq!(std::fs::read(&destination).unwrap(), b"original");
  let output = cli(dir.path())
    .arg("download")
    .arg(seed)
    .arg("-o")
    .arg(dir.path().join("missing"))
    .arg("--resume")
    .output()
    .unwrap();
  assert_eq!(output.status.code(), Some(1));
  assert!(!dir.path().join("missing").exists());
}

#[test]
fn explicit_flags_override_environment() {
  let dir = TempDir::new().unwrap();
  let seed = empty_seed(dir.path());
  let output = cli(dir.path())
    .env("Q2Q_TRACK_URL", "invalid-url")
    .env("Q2Q_HOME", dir.path().join("wrong-state"))
    .env("Q2Q_IDENTITY_FILE", dir.path().join("wrong-identity"))
    .arg("--identity")
    .arg(dir.path().join("chosen-identity"))
    .args(["--track", "http://127.0.0.1:1", "download"])
    .arg(seed)
    .arg("-o")
    .arg(dir.path().join("output"))
    .output()
    .unwrap();
  success(&output);
  assert!(dir.path().join("chosen-identity").exists());
  assert!(dir.path().join("wrong-state").is_dir());
  assert!(!dir.path().join("wrong-identity").exists());
}

#[cfg(unix)]
#[path = "local.rs"]
mod local;
