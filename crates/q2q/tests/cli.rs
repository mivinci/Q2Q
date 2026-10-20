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

#[test]
fn magnet_export_is_local_and_has_a_single_line_stdout() {
  let dir = TempDir::new().unwrap();
  let path = empty_seed(dir.path());
  let output = cli(dir.path()).arg("magnet").arg(&path).output().unwrap();
  success(&output);
  let expected =
    q2q_primitives::seed::to_magnet(&Seed::parse(&std::fs::read(path).unwrap()).unwrap());
  assert_eq!(
    String::from_utf8(output.stdout).unwrap(),
    format!("{expected}\n")
  );
  assert!(output.stderr.is_empty());
  assert!(!dir.path().join("state").exists());
}

#[test]
fn bad_magnet_never_creates_state_or_destination() {
  let dir = TempDir::new().unwrap();
  let output_path = dir.path().join("must-not-exist");
  for input in [
    "magnet:bad".into(),
    "magnet:?xt=urn:btih:abcd".into(),
    format!("magnet:?xt=urn:sha256:{}", "aé".repeat(21) + "a"),
  ] {
    let output = cli(dir.path())
      .arg("download")
      .arg(input)
      .arg("-o")
      .arg(&output_path)
      .output()
      .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(!output_path.exists());
    assert!(!dir.path().join("state").exists());
  }
}

#[test]
fn cached_magnet_uses_verified_name_or_hash_and_resumes_without_identity() {
  for name in [None, Some("../../unsafe"), Some("safe-photo.jpeg")] {
    let dir = TempDir::new().unwrap();
    let mut info = Info::new(0, EMPTY_ROOT);
    info.name = name.map(String::from);
    let seed = Seed::new(info);
    let hash: String = seed
      .info_hash()
      .iter()
      .map(|b| format!("{b:02x}"))
      .collect();
    let link = format!(
      "{}&dn=untrusted-name",
      q2q_primitives::seed::to_magnet(&seed)
    );
    let metadata = dir.path().join("state/metadata");
    std::fs::create_dir_all(&metadata).unwrap();
    std::fs::write(metadata.join(format!("{hash}.q2q")), seed.encode()).unwrap();
    let output = cli(dir.path())
      .current_dir(dir.path())
      .args(["--quiet", "download", &link])
      .output()
      .unwrap();
    assert!(
      output.status.success(),
      "name={name:?}, output={output:?}, files={:?}",
      std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect::<Vec<_>>()
    );
    let expected_name = name.filter(|n| !n.contains('/')).unwrap_or(&hash);
    assert_eq!(
      String::from_utf8_lossy(&output.stdout).trim(),
      expected_name
    );
    assert!(dir.path().join(expected_name).is_file());
    assert!(output.stderr.is_empty());
    // Offline completion must not reopen a corrupt identity or contact Track.
    std::fs::write(dir.path().join("state/identity"), b"corrupt identity").unwrap();
    let output = cli(dir.path())
      .current_dir(dir.path())
      .args([
        "--track",
        "http://127.0.0.1:1",
        "--quiet",
        "download",
        &link,
      ])
      .output()
      .unwrap();
    success(&output);
  }
}

#[test]
fn seed_command_hashes_locally_without_state_or_network_and_refuses_overwrites() {
  let dir = TempDir::new().unwrap();
  let source = dir.path().join("photo.bin");
  let bytes = vec![37; 524_301];
  std::fs::write(&source, &bytes).unwrap();
  let output = cli(dir.path())
    .env("Q2Q_HOME", "")
    .args(["--quiet", "--track", "http://127.0.0.1:1", "seed"])
    .arg(&source)
    .output()
    .unwrap();
  success(&output);
  let path = dir.path().join("photo.bin.q2q");
  assert_eq!(
    String::from_utf8_lossy(&output.stdout).trim(),
    path.display().to_string()
  );
  let seed_bytes = std::fs::read(&path).unwrap();
  let seed = Seed::parse(&seed_bytes).unwrap();
  assert_eq!(seed.info.length, bytes.len() as u64);
  assert_eq!(
    seed.info.root,
    q2q_primitives::merkle::root(
      &bytes
        .chunks(262144)
        .map(q2q_primitives::merkle::leaf_hash)
        .collect::<Vec<_>>()
    )
  );
  assert_eq!(seed.info.name.as_deref(), Some("photo.bin"));
  assert!(output.stderr.is_empty());
  assert!(!dir.path().join("state").exists());
  let again = cli(dir.path()).arg("seed").arg(&source).output().unwrap();
  assert_eq!(again.status.code(), Some(1));
  assert_eq!(std::fs::read(&path).unwrap(), seed_bytes);
  let custom = dir.path().join("custom.q2q");
  success(
    &cli(dir.path())
      .arg("seed")
      .arg(&source)
      .arg("-o")
      .arg(&custom)
      .output()
      .unwrap(),
  );
  assert!(!dir.path().join("state").exists());
}

#[test]
fn publish_seed_rejects_unsafe_or_mismatched_sources_before_creating_identity() {
  let dir = TempDir::new().unwrap();
  let path = dir.path().join("seed.q2q");
  let source = dir.path().join("data.bin");
  std::fs::write(&source, b"wrong bytes").unwrap();
  for name in [None, Some("../../outside"), Some("data.bin")] {
    let mut seed = Seed::new(Info::new(0, EMPTY_ROOT));
    seed.info.name = name.map(String::from);
    std::fs::write(&path, seed.encode()).unwrap();
    let before = std::fs::read(&path).unwrap();
    let result = cli(dir.path())
      .args(["--track", "http://127.0.0.1:1", "publish"])
      .arg(&path)
      .output()
      .unwrap();
    assert_eq!(result.status.code(), Some(1));
    assert!(result.stdout.is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(!dir.path().join("state").exists());
  }
  let result = cli(dir.path())
    .arg("publish")
    .arg(&path)
    .arg("--file")
    .arg(&source)
    .output()
    .unwrap();
  assert_eq!(result.status.code(), Some(1));
  assert!(String::from_utf8_lossy(&result.stderr).contains("does not match"));
  assert!(!dir.path().join("state").exists());
}
