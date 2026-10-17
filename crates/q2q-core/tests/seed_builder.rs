use q2q_core::merkle;
use q2q_core::seed::{Seed, DEFAULT_BLOCK_SIZE};
use q2q_core::seed_builder::{BuildError, SeedBuilder};

#[test]
fn chunk_boundaries_do_not_change_seed_or_proofs() {
  let size = DEFAULT_BLOCK_SIZE as usize;
  for length in [0, 1, size - 1, size, size + 1, size * 3 + 17] {
    let bytes: Vec<_> = (0..length).map(|n| (n % 251) as u8).collect();
    let leaves: Vec<_> = bytes.chunks(size).map(merkle::leaf_hash).collect();
    for chunk_size in [1, 997, size, size + 19] {
      let mut builder = SeedBuilder::new(length as u64).unwrap();
      builder.update(&[]).unwrap();
      for chunk in bytes.chunks(chunk_size) {
        builder.update(chunk).unwrap();
      }
      assert_eq!(builder.received(), length as u64);
      let seed = builder.finish().unwrap();
      assert_eq!(seed.info.root, merkle::root(&leaves));
      assert_eq!(seed.leaf_hashes.as_ref().unwrap(), &leaves);
      assert_eq!(Seed::parse(&seed.encode()).unwrap(), seed);
    }
  }
}

#[test]
fn overflow_update_is_atomic_and_short_input_cannot_finish() {
  let mut builder = SeedBuilder::new(3).unwrap();
  builder.update(b"a").unwrap();
  assert_eq!(builder.update(b"bcd"), Err(BuildError::LengthExceeded));
  assert_eq!(builder.received(), 1);
  builder.update(b"bc").unwrap();
  assert_eq!(
    builder.finish().unwrap().info.root,
    merkle::leaf_hash(b"abc")
  );
  assert!(matches!(
    SeedBuilder::new(1).unwrap().finish(),
    Err(BuildError::LengthMismatch)
  ));
}

#[test]
fn validates_format_limits_and_custom_block_size() {
  assert!(matches!(
    SeedBuilder::new(u64::MAX),
    Err(BuildError::LengthOutOfRange)
  ));
  for size in [
    0,
    1,
    DEFAULT_BLOCK_SIZE - 1,
    3 * DEFAULT_BLOCK_SIZE,
    8 * 1024 * 1024,
  ] {
    assert!(matches!(
      SeedBuilder::with_block_size(1, size),
      Err(BuildError::InvalidBlockSize)
    ));
  }
  let block = DEFAULT_BLOCK_SIZE * 2;
  let bytes = vec![42; block as usize + 1];
  let mut builder = SeedBuilder::with_block_size(bytes.len() as u64, block).unwrap();
  builder.update(&bytes).unwrap();
  let seed = builder.finish().unwrap();
  assert_eq!(seed.info.block_size, block);
  assert_eq!(seed.leaf_hashes.unwrap().len(), 2);
}
