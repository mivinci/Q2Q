use super::*;
use crate::publication::PreparedFile;
use q2q_core::block::BlockState;
use std::io::{Seek, SeekFrom};

fn fixture(dir: &Path) -> (PreparedFile, Vec<u8>, PathBuf) {
  let bytes: Vec<_> = (0..262144 + 31).map(|i| (i % 251) as u8).collect();
  let source = dir.join("source.bin");
  std::fs::write(&source, &bytes).unwrap();
  let prepared = PreparedFile::open(source, "http://localhost".into()).unwrap();
  (prepared, bytes, dir.join("download.bin"))
}

fn commit(store: &mut DownloadStore, source: &PreparedFile, bytes: &[u8], index: usize) {
  let start = index * 262144;
  let end = (start + 262144).min(bytes.len());
  store
    .commit(
      index,
      &bytes[start..end],
      source.proof(index).unwrap(),
      &source.seed().info.root,
    )
    .unwrap();
}

#[test]
fn resumed_blocks_are_rehashed_and_old_state_flags_are_ignored() {
  let dir = tempfile::tempdir().unwrap();
  let (source, bytes, path) = fixture(dir.path());
  let seed = source.seed();
  let mut store = DownloadStore::create(seed, &path).unwrap();
  commit(&mut store, &source, &bytes, 0);
  commit(&mut store, &source, &bytes, 1);
  store.save_progress().unwrap();
  store.save_leaves(&seed.info.root).unwrap();
  drop(store);
  let mut file = OpenOptions::new().write(true).open(&path).unwrap();
  file.seek(SeekFrom::Start(17)).unwrap();
  file.write_all(&[255]).unwrap();
  drop(file);
  let mut resumed = DownloadStore::resume(seed, &path).unwrap();
  assert_ne!(resumed.state(0).unwrap(), BlockState::Verified);
  assert_eq!(resumed.state(1).unwrap(), BlockState::Verified);
  commit(&mut resumed, &source, &bytes, 0);
  drop(resumed);
  let resumed = DownloadStore::resume(seed, &path).unwrap();
  assert_eq!(resumed.verified_bitfield(), vec![0xc0]);
  assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[test]
fn torn_append_is_removed_and_receipts_remain_appendable() {
  let dir = tempfile::tempdir().unwrap();
  let (source, bytes, path) = fixture(dir.path());
  let mut store = DownloadStore::create(source.seed(), &path).unwrap();
  commit(&mut store, &source, &bytes, 0);
  drop(store);
  OpenOptions::new()
    .append(true)
    .open(sidecar(&path, ".q2q.resume"))
    .unwrap()
    .write_all(b"{\"index\":1,\"proof\":")
    .unwrap();
  let mut store = DownloadStore::resume(source.seed(), &path).unwrap();
  assert_eq!(store.verified_bitfield(), vec![0x80]);
  commit(&mut store, &source, &bytes, 1);
  drop(store);
  assert_eq!(
    DownloadStore::resume(source.seed(), &path)
      .unwrap()
      .verified_bitfield(),
    vec![0xc0]
  );
}

#[test]
fn forged_proof_is_not_restored_even_when_header_matches() {
  let dir = tempfile::tempdir().unwrap();
  let (source, bytes, path) = fixture(dir.path());
  let mut store = DownloadStore::create(source.seed(), &path).unwrap();
  commit(&mut store, &source, &bytes, 0);
  drop(store);
  let journal = sidecar(&path, ".q2q.resume");
  let mut lines: Vec<_> = std::fs::read_to_string(&journal)
    .unwrap()
    .lines()
    .map(str::to_owned)
    .collect();
  let mut receipt: Receipt = serde_json::from_str(&lines[1]).unwrap();
  receipt.proof.nodes[0].hash[0] ^= 1;
  lines[1] = serde_json::to_string(&receipt).unwrap();
  std::fs::write(journal, lines.join("\n") + "\n").unwrap();
  let restored = DownloadStore::resume(source.seed(), &path).unwrap();
  assert_eq!(restored.verified_bitfield(), vec![0]);
  assert_eq!(&std::fs::read(path).unwrap()[..262144], &bytes[..262144]);
}

#[test]
fn mismatched_seed_and_malformed_receipts_leave_files_unchanged() {
  let dir = tempfile::tempdir().unwrap();
  let (source, bytes, path) = fixture(dir.path());
  let mut store = DownloadStore::create(source.seed(), &path).unwrap();
  commit(&mut store, &source, &bytes, 0);
  drop(store);
  let journal = sidecar(&path, ".q2q.resume");
  let original = std::fs::read(&journal).unwrap();
  let destination = std::fs::read(&path).unwrap();
  let mut seed = source.seed().clone();
  seed.info.name = Some("another file".into());
  assert!(matches!(
    DownloadStore::resume(&seed, &path),
    Err(TransferError::Checkpoint(_))
  ));
  assert_eq!(std::fs::read(&journal).unwrap(), original);
  for suffix in [
    original.split(|b| *b == b'\n').nth(1).unwrap().to_vec(),
    b"{\"index\":99,\"proof\":{\"leaf_hashes\":[],\"nodes\":[]}}".to_vec(),
    b"not-json".to_vec(),
    vec![b'x'; MAX_CONTROL_BYTES + 2],
  ] {
    let mut altered = original.clone();
    altered.extend(suffix);
    altered.push(b'\n');
    std::fs::write(&journal, &altered).unwrap();
    assert!(matches!(
      DownloadStore::resume(source.seed(), &path),
      Err(TransferError::Checkpoint(_))
    ));
    assert_eq!(std::fs::read(&journal).unwrap(), altered);
    assert_eq!(std::fs::read(&path).unwrap(), destination);
  }
}

#[test]
fn checkpoint_write_failure_prevents_later_queued_writes() {
  let dir = tempfile::tempdir().unwrap();
  let (source, bytes, path) = fixture(dir.path());
  let mut store = DownloadStore::create(source.seed(), &path).unwrap();
  let journal = sidecar(&path, ".q2q.resume");
  // A read-only descriptor makes the receipt write fail after data was synced.
  store.journal = File::open(&journal).unwrap();
  assert!(
    store
      .commit(
        0,
        &bytes[..262144],
        source.proof(0).unwrap(),
        &source.seed().info.root
      )
      .is_err()
  );
  store.journal = OpenOptions::new().append(true).open(&journal).unwrap();
  assert!(matches!(
    store.commit(
      1,
      &bytes[262144..],
      source.proof(1).unwrap(),
      &source.seed().info.root
    ),
    Err(TransferError::Checkpoint(_))
  ));
  assert!(store.read_block(1).unwrap().iter().all(|byte| *byte == 0));
  drop(store);
  assert_eq!(
    DownloadStore::resume(source.seed(), &path)
      .unwrap()
      .verified_bitfield(),
    vec![0]
  );
}

#[test]
fn destination_lock_and_create_new_preserve_existing_files() {
  let dir = tempfile::tempdir().unwrap();
  let (source, _, path) = fixture(dir.path());
  let store = DownloadStore::create(source.seed(), &path).unwrap();
  assert!(matches!(
    DownloadStore::resume(source.seed(), &path),
    Err(TransferError::Checkpoint(_))
  ));
  assert!(
    matches!(DownloadStore::create(source.seed(), &path), Err(TransferError::Io(ref e)) if e.kind() == std::io::ErrorKind::AlreadyExists)
  );
  drop(store);
  let resumed = DownloadStore::resume(source.seed(), &path).unwrap();
  drop(resumed);
  std::fs::remove_file(&path).unwrap();
  assert!(DownloadStore::create(source.seed(), &path).is_err());
  assert!(!path.exists());
}

#[test]
fn abrupt_process_exit_releases_lock_and_preserves_synced_receipts() {
  let dir = tempfile::tempdir().unwrap();
  let (source, _, path) = fixture(dir.path());
  let status = std::process::Command::new(std::env::current_exe().unwrap())
    .args(["--exact", "download_store::tests::resume_child_process"])
    .env("Q2Q_SDK_RESUME_TEST_DIR", dir.path())
    .status()
    .unwrap();
  assert!(status.success());
  let restored = DownloadStore::resume(source.seed(), &path).unwrap();
  assert_eq!(restored.verified_bitfield(), vec![0x80]);
}

#[test]
fn resume_child_process() {
  let Some(dir) = std::env::var_os("Q2Q_SDK_RESUME_TEST_DIR") else {
    return;
  };
  let (source, bytes, path) = fixture(Path::new(&dir));
  let mut store = DownloadStore::create(source.seed(), &path).unwrap();
  commit(&mut store, &source, &bytes, 0);
  std::process::exit(0);
}

#[test]
fn missing_or_wrong_version_checkpoint_and_wrong_length_are_rejected() {
  for mode in 0..4 {
    let dir = tempfile::tempdir().unwrap();
    let (source, _, path) = fixture(dir.path());
    drop(DownloadStore::create(source.seed(), &path).unwrap());
    let journal = sidecar(&path, ".q2q.resume");
    match mode {
      0 => std::fs::remove_file(&journal).unwrap(),
      1 => {
        let mut header: Header = serde_json::from_slice(&std::fs::read(&journal).unwrap()).unwrap();
        header.version = 99;
        let mut file = File::create(&journal).unwrap();
        write_record(&mut file, &header).unwrap();
      }
      2 => OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(1)
        .unwrap(),
      _ => std::fs::write(&journal, b"{\"version\":").unwrap(),
    }
    let before = std::fs::read(&path).unwrap();
    assert!(DownloadStore::resume(source.seed(), &path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
  }
}

#[tokio::test]
async fn empty_destination_resumes_offline_with_completed_zero_progress() {
  let dir = tempfile::tempdir().unwrap();
  let seed = Seed::new(q2q_core::seed::Info::new(0, q2q_core::merkle::EMPTY_ROOT));
  let path = dir.path().join("empty.bin");
  drop(DownloadStore::create(&seed, &path).unwrap());
  let client = crate::client::builder()
    .track_url("http://127.0.0.1:1")
    .identity_file(dir.path().join("identity"))
    .storage_dir(dir.path().join("storage"))
    .build()
    .await
    .unwrap();
  let transfer = client.resume_download(&seed, &path).await.unwrap();
  let observed = transfer.subscribe_progress();
  transfer.wait().await.unwrap();
  assert_eq!(observed.borrow().verified_bytes, 0);
  assert_eq!(observed.borrow().total_blocks, 0);
  assert_eq!(
    observed.borrow().state,
    crate::transfer::TransferState::Completed
  );
}

#[tokio::test]
async fn completed_resume_uses_public_api_without_track() {
  let dir = tempfile::tempdir().unwrap();
  let (source, bytes, path) = fixture(dir.path());
  let mut store = DownloadStore::create(source.seed(), &path).unwrap();
  commit(&mut store, &source, &bytes, 0);
  commit(&mut store, &source, &bytes, 1);
  drop(store);
  let client = crate::client::builder()
    .track_url("http://127.0.0.1:1")
    .identity_file(dir.path().join("identity"))
    .storage_dir(dir.path().join("storage"))
    .build()
    .await
    .unwrap();
  let transfer = client.resume_download(source.seed(), &path).await.unwrap();
  assert_eq!(transfer.progress().verified_bytes, bytes.len() as u64);
  let observed = transfer.subscribe_progress();
  let result = transfer.wait().await.unwrap();
  assert_eq!(
    observed.borrow().state,
    crate::transfer::TransferState::Completed
  );
  assert_eq!(result, path);
  assert_eq!(std::fs::read(result).unwrap(), bytes);
}

pub(crate) fn make_receipts_read_only(store: &mut DownloadStore, path: &Path) {
  store.journal = File::open(sidecar(path, ".q2q.resume")).unwrap();
}
