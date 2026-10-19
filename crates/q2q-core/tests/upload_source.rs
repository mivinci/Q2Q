use super::*;
use crate::{block_queue::WorkQueue, destination::Destination, source_file::PreparedFile};
use std::{path::Path, time::Duration};

fn fixture(dir: &Path) -> (PreparedFile, Vec<u8>) {
  let bytes: Vec<_> = (0..2 * 262144 + 13).map(|i| (i % 251) as u8).collect();
  let path = dir.join("source");
  std::fs::write(&path, &bytes).unwrap();
  (
    PreparedFile::open(path, "http://localhost".into()).unwrap(),
    bytes,
  )
}
async fn commit(store: &Destination, source: &PreparedFile, bytes: &[u8], index: usize) {
  let bits = store.bitfield().await.unwrap();
  let progress = crate::progress::channel(source.seed(), &bits);
  let queue = WorkQueue::new(source.seed(), &bits, progress);
  let mut offered = vec![0; bits.len()];
  offered[index / 8] |= 1 << (7 - index % 8);
  let lease = queue.claim_available(&offered).unwrap();
  let start = index * 262144;
  let end = (start + 262144).min(bytes.len());
  store
    .commit(
      lease,
      bytes[start..end].to_vec(),
      source.proof(index).unwrap(),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn committed_blocks_notify_and_share_proofs_without_completing_early() {
  let dir = tempfile::tempdir().unwrap();
  let (source, bytes) = fixture(dir.path());
  let path = dir.path().join("destination");
  let store = Destination::open(source.seed(), &path, false)
    .await
    .unwrap();
  let reads = Arc::new(Semaphore::new(2));
  let upload = store.upload_source(reads.clone());
  assert!(Arc::ptr_eq(&reads, &upload.read_limit()));
  let mut availability = upload.availability();
  assert!(!availability.borrow().any());
  for index in [2, 0, 1] {
    commit(&store, &source, &bytes, index).await;
    availability.changed().await.unwrap();
    assert!(availability.borrow_and_update().has(index));
    assert!(!availability.borrow().complete);
    let (data, proof) = upload.read_block(index).unwrap();
    assert!(q2q_primitives::merkle::verify(
      &data,
      index,
      3,
      &proof,
      &source.seed().info.root
    ));
  }
  store.finish().await.unwrap();
  assert!(availability.borrow().complete);
  drop(upload);
  drop(store);
  let resumed = Destination::open(source.seed(), &path, true).await.unwrap();
  let upload = resumed.upload_source(reads);
  assert_eq!(upload.availability().borrow().bits, vec![0xe0]);
  for index in 0..3 {
    assert_eq!(
      upload.read_block(index).unwrap().0,
      source.read_verified_block(index).unwrap()
    );
  }
}

#[tokio::test]
async fn failed_checkpoint_never_advertises_written_but_uncommitted_bytes() {
  let dir = tempfile::tempdir().unwrap();
  let (source, bytes) = fixture(dir.path());
  let path = dir.path().join("destination");
  let store = Destination::open(source.seed(), &path, false)
    .await
    .unwrap();
  let upload = store.upload_source(Arc::new(Semaphore::new(1)));
  crate::download_store::tests::make_receipts_read_only(&mut store.lock().unwrap(), &path);
  let queue = WorkQueue::new(
    source.seed(),
    &[0],
    crate::progress::channel(source.seed(), &[0]),
  );
  let lease = queue.claim_available(&[0x80]).unwrap();
  assert!(
    store
      .commit(lease, bytes[..262144].to_vec(), source.proof(0).unwrap())
      .await
      .is_err()
  );
  assert!(!upload.availability().borrow().has(0));
  assert!(upload.availability().borrow().failed);
  assert!(upload.read_block(0).is_err());
}

#[tokio::test]
async fn resumed_corrupt_block_is_not_shared_and_active_tampering_stops_source() {
  use std::io::{Seek, SeekFrom, Write};
  let dir = tempfile::tempdir().unwrap();
  let (source, bytes) = fixture(dir.path());
  let path = dir.path().join("destination");
  let store = Destination::open(source.seed(), &path, false)
    .await
    .unwrap();
  commit(&store, &source, &bytes, 0).await;
  commit(&store, &source, &bytes, 2).await;
  let upload = store.upload_source(Arc::new(Semaphore::new(1)));
  let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
  file.seek(SeekFrom::Start(0)).unwrap();
  file.write_all(&[255]).unwrap();
  assert!(upload.read_block(0).is_err());
  assert!(upload.availability().borrow().failed);
  drop(upload);
  drop(store);
  let resumed = Destination::open(source.seed(), &path, true).await.unwrap();
  let upload = resumed.upload_source(Arc::new(Semaphore::new(1)));
  assert!(!upload.availability().borrow().has(0));
  assert!(upload.availability().borrow().has(2));
  assert!(!upload.availability().borrow().failed);
}

#[tokio::test]
async fn partial_peer_sends_have_and_serves_new_blocks_on_the_same_connection() {
  let dir = tempfile::tempdir().unwrap();
  let (source, bytes) = fixture(dir.path());
  let store = Destination::open(source.seed(), &dir.path().join("destination"), false)
    .await
    .unwrap();
  commit(&store, &source, &bytes, 0).await;
  let (download, upload) = crate::peer::test_pair(source.seed().info_hash()).await;
  let serving = tokio::spawn(upload.serve_file_scoped(
    store.upload_source(Arc::new(Semaphore::new(1))),
    crate::upload_session::UploadContext::unmanaged(),
  ));
  let mut connection = download
    .begin_download(source.seed(), vec![0])
    .await
    .unwrap();
  assert_eq!(connection.available(), vec![0x80]);
  let (data, proof) = connection.fetch(0, 262144).await.unwrap();
  assert_eq!(data, bytes[..262144]);
  assert!(q2q_primitives::merkle::verify(
    &data,
    0,
    3,
    &proof,
    &source.seed().info.root
  ));
  connection.acknowledge(0).await.unwrap();
  commit(&store, &source, &bytes, 2).await;
  tokio::time::timeout(Duration::from_secs(2), connection.receive_have())
    .await
    .unwrap()
    .unwrap();
  assert_eq!(connection.available(), vec![0xa0]);
  let (data, _) = connection.fetch(2, 13).await.unwrap();
  assert_eq!(data, bytes[2 * 262144..]);
  connection.acknowledge(2).await.unwrap();
  connection.finish().await.unwrap();
  serving.await.unwrap().unwrap();
}

#[tokio::test]
async fn download_guard_withdraws_cancelled_content_but_preserves_successful_content() {
  let dir = tempfile::tempdir().unwrap();
  let (source, _) = fixture(dir.path());
  let store = Destination::open(source.seed(), &dir.path().join("cancelled"), false)
    .await
    .unwrap();
  let guard = crate::destination::DownloadGuard::new(store.clone());
  drop(guard);
  assert!(
    store
      .upload_source(Arc::new(Semaphore::new(1)))
      .availability()
      .borrow()
      .failed
  );
  let store = Destination::open(source.seed(), &dir.path().join("finished"), false)
    .await
    .unwrap();
  let mut guard = crate::destination::DownloadGuard::new(store.clone());
  guard.finish();
  drop(guard);
  assert!(
    !store
      .upload_source(Arc::new(Semaphore::new(1)))
      .availability()
      .borrow()
      .failed
  );
}
