use super::*;

#[test]
fn existing_seed_preserves_identity_with_renamed_source_and_custom_blocks() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("renamed.bin");
  let bytes: Vec<_> = (0..1_100_001).map(|i| (i % 251) as u8).collect();
  std::fs::write(&path, &bytes).unwrap();
  let block_size = 512 * 1024;
  let mut builder = SeedBuilder::with_block_size(bytes.len() as u64, block_size).unwrap();
  builder.update(&bytes).unwrap();
  let mut seed = builder.finish().unwrap();
  seed.info.name = Some("original.bin".into());
  seed.info.mime = Some("application/example".into());
  seed.cdn_list.push("https://cdn.example/content".into());
  seed.leaf_hashes = None;
  let mut value = seed.info.to_value();
  let q2q_primitives::bencode::BValue::Dict(ref mut fields) = value else {
    panic!()
  };
  fields.insert(b"future".to_vec(), q2q_primitives::bencode::BValue::Int(7));
  seed.info = q2q_primitives::seed::Info::parse_verified(
    &value.encode(),
    &q2q_primitives::bencode::digest(&value),
  )
  .unwrap();
  let prepared = PreparedFile::from_seed(path.clone(), seed.clone()).unwrap();
  assert_eq!(prepared.seed().info, seed.info);
  assert_eq!(prepared.seed().cdn_list, seed.cdn_list);
  for (index, chunk) in bytes.chunks(block_size as usize).enumerate() {
    assert_eq!(prepared.read_verified_block(index).unwrap(), chunk);
    assert!(merkle::verify(
      chunk,
      index,
      prepared.block_count(),
      &prepared.proof(index).unwrap(),
      &seed.info.root
    ));
  }
  let mut wrong = seed.clone();
  wrong.info.length += 1;
  assert!(matches!(
    PreparedFile::from_seed(path.clone(), wrong),
    Err(PrepareError::SeedMismatch)
  ));
  let mut wrong = seed;
  wrong.info.root = [0; 32];
  assert!(matches!(
    PreparedFile::from_seed(path, wrong),
    Err(PrepareError::SeedMismatch)
  ));
}

#[test]
fn prepared_seed_preserves_the_existing_file_identity_and_sources() {
  let directory = tempfile::tempdir().unwrap();
  let path = directory.path().join("identity.bin");
  let bytes = vec![37; DEFAULT_BLOCK_SIZE as usize * 3 + 5];
  std::fs::write(&path, &bytes).unwrap();
  let prepared = PreparedFile::open(path, "https://track.example".into()).unwrap();
  let leaves: Vec<_> = bytes
    .chunks(DEFAULT_BLOCK_SIZE as usize)
    .map(merkle::leaf_hash)
    .collect();
  let mut info = q2q_primitives::seed::Info::new(bytes.len() as u64, merkle::root(&leaves));
  info.name = Some("identity.bin".into());
  let mut expected = Seed::new(info);
  expected.track_list.push("https://track.example".into());
  expected.leaf_hashes = Some(leaves);
  assert_eq!(prepared.seed().encode(), expected.encode());
  assert_eq!(prepared.seed().info_hash(), expected.info_hash());
}

#[test]
fn positional_reads_are_shared_and_verify_every_block() {
  let directory = tempfile::tempdir().unwrap();
  let path = directory.path().join("source");
  let bytes: Vec<_> = (0..3 * DEFAULT_BLOCK_SIZE as usize + 13)
    .map(|i| (i % 251) as u8)
    .collect();
  std::fs::write(&path, &bytes).unwrap();
  let prepared = PreparedFile::open(path, "http://localhost".into()).unwrap();
  std::thread::scope(|scope| {
    for worker in 0..8 {
      let source = &prepared;
      let expected = &bytes;
      scope.spawn(move || {
        for iteration in 0..16 {
          let index = (iteration * 3 + worker) % 4;
          let start = index * DEFAULT_BLOCK_SIZE as usize;
          let end = (start + DEFAULT_BLOCK_SIZE as usize).min(expected.len());
          assert_eq!(
            source.read_verified_block(index).unwrap(),
            expected[start..end]
          );
        }
      });
    }
  });
}

#[test]
fn clone_shares_file_tree_seed_and_budget_without_reopening_the_path() {
  let directory = tempfile::tempdir().unwrap();
  let path = directory.path().join("source");
  std::fs::write(&path, [7; 1024]).unwrap();
  let source = PreparedFile::open(path, "http://localhost".into()).unwrap();
  let other = source.clone();
  assert!(Arc::ptr_eq(&source.file, &other.file));
  assert!(Arc::ptr_eq(&source.seed, &other.seed));
  assert!(Arc::ptr_eq(&source.tree, &other.tree));
  assert!(Arc::ptr_eq(&source.reads, &other.reads));
}

#[cfg(unix)]
#[test]
fn concurrent_reads_do_not_use_or_move_the_shared_cursor() {
  use std::io::{Seek, SeekFrom};
  let directory = tempfile::tempdir().unwrap();
  let path = directory.path().join("source");
  std::fs::write(&path, vec![7; 2 * DEFAULT_BLOCK_SIZE as usize]).unwrap();
  let source = PreparedFile::open(path, "http://localhost".into()).unwrap();
  let mut cursor = &*source.file;
  cursor.seek(SeekFrom::Start(11)).unwrap();
  source.read_verified_block(1).unwrap();
  source.read_verified_block(0).unwrap();
  assert_eq!(cursor.stream_position().unwrap(), 11);
}

#[test]
fn leaf_verification_detects_tampering_even_if_timestamp_is_restored() {
  use std::io::Write;
  let directory = tempfile::tempdir().unwrap();
  let path = directory.path().join("source");
  std::fs::write(&path, [7; 1024]).unwrap();
  let source = PreparedFile::open(path.clone(), "http://localhost".into()).unwrap();
  let mut writer = std::fs::OpenOptions::new().write(true).open(path).unwrap();
  writer.write_all(&[9]).unwrap();
  if let Some(modified) = source.modified {
    writer
      .set_times(std::fs::FileTimes::new().set_modified(modified))
      .unwrap();
  }
  assert!(matches!(
    source.read_verified_block(0),
    Err(PrepareError::SourceChanged)
  ));
}

#[test]
fn length_changes_and_invalid_ranges_fail_without_serving_data() {
  let directory = tempfile::tempdir().unwrap();
  for length in [1, 2048] {
    let path = directory.path().join(format!("source-{length}"));
    std::fs::write(&path, [7; 1024]).unwrap();
    let source = PreparedFile::open(path.clone(), "http://localhost".into()).unwrap();
    assert!(matches!(
      source.read_verified_block(usize::MAX),
      Err(PrepareError::BlockOutOfRange(_))
    ));
    std::fs::OpenOptions::new()
      .write(true)
      .open(path)
      .unwrap()
      .set_len(length)
      .unwrap();
    assert!(matches!(
      source.read_verified_block(0),
      Err(PrepareError::SourceChanged)
    ));
  }
  let path = directory.path().join("empty");
  std::fs::write(&path, []).unwrap();
  let empty = PreparedFile::open(path, "http://localhost".into()).unwrap();
  assert!(matches!(
    empty.read_verified_block(0),
    Err(PrepareError::BlockOutOfRange(0))
  ));
}

#[cfg(unix)]
#[test]
fn replacing_path_does_not_replace_the_open_source() {
  let directory = tempfile::tempdir().unwrap();
  let path = directory.path().join("source");
  std::fs::write(&path, [7; 1024]).unwrap();
  let source = PreparedFile::open(path.clone(), "http://localhost".into()).unwrap();
  std::fs::rename(&path, directory.path().join("original")).unwrap();
  std::fs::write(path, [9; 1024]).unwrap();
  assert_eq!(
    source.clone().read_verified_block(0).unwrap(),
    vec![7; 1024]
  );
}

#[cfg(any(unix, windows))]
#[test]
fn short_and_interrupted_reads_preserve_offsets_and_eof_is_an_error() {
  let mut buffer = [0; 5];
  let mut first = true;
  let mut offsets = Vec::new();
  read_exact_with(&mut buffer, 7, |data, offset| {
    offsets.push(offset);
    if first {
      first = false;
      return Err(io::ErrorKind::Interrupted.into());
    }
    let count = data.len().min(2);
    data[..count].fill(offset as u8);
    Ok(count)
  })
  .unwrap();
  assert_eq!(offsets, vec![7, 7, 9, 11]);
  assert_eq!(buffer, [7, 7, 9, 9, 11]);
  assert_eq!(
    read_exact_with(&mut buffer, 0, |_, _| Ok(0))
      .unwrap_err()
      .kind(),
    io::ErrorKind::UnexpectedEof
  );
}

#[tokio::test]
async fn clones_share_a_bounded_cancelable_read_gate() {
  let directory = tempfile::tempdir().unwrap();
  let path = directory.path().join("source");
  std::fs::write(&path, [7; 1024]).unwrap();
  let reads = Arc::new(Semaphore::new(2));
  let source = PreparedFile::open(path, "http://localhost".into())
    .unwrap()
    .with_read_limit(reads.clone());
  let other = source.clone();
  let first = source.read_permit().await;
  let second = other.read_permit().await;
  assert!(
    tokio::time::timeout(std::time::Duration::from_millis(10), source.read_permit())
      .await
      .is_err()
  );
  drop(first);
  let third = other.read_permit().await;
  assert_eq!(reads.available_permits(), 0);
  drop((second, third));
  assert_eq!(reads.available_permits(), 2);
}
