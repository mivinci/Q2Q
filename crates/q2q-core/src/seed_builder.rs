//! Incremental content hashing without file, network or runtime dependencies.
use crate::merkle::{self, Hash};
use crate::seed::{Info, Seed, DEFAULT_BLOCK_SIZE};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildError {
  LengthOutOfRange,
  InvalidBlockSize,
  LengthExceeded,
  LengthMismatch,
  TooManyBlocks,
}

impl std::fmt::Display for BuildError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.write_str(match self {
      Self::LengthOutOfRange => "length exceeds the seed format's signed integer range",
      Self::InvalidBlockSize => "invalid seed block size",
      Self::LengthExceeded => "input exceeds the declared content length",
      Self::LengthMismatch => "input does not match the declared content length",
      Self::TooManyBlocks => "cannot allocate the content leaf table",
    })
  }
}
impl std::error::Error for BuildError {}

/// Hashes each protocol block once, regardless of the caller's chunk boundaries.
/// Memory holds a hash state and the leaf table, never the content bytes.
pub struct SeedBuilder {
  length: u64,
  received: u64,
  block_size: u32,
  block_received: usize,
  hash: Sha256,
  leaves: Vec<Hash>,
}

impl SeedBuilder {
  pub fn new(length: u64) -> Result<Self, BuildError> {
    Self::with_block_size(length, DEFAULT_BLOCK_SIZE)
  }

  pub fn with_block_size(length: u64, block_size: u32) -> Result<Self, BuildError> {
    if length > i64::MAX as u64 {
      return Err(BuildError::LengthOutOfRange);
    }
    let mut info = Info::new(length, merkle::EMPTY_ROOT);
    info.block_size = block_size;
    info.validate().map_err(|_| BuildError::InvalidBlockSize)?;
    usize::try_from(length.div_ceil(block_size as u64)).map_err(|_| BuildError::TooManyBlocks)?;
    let hash = merkle::leaf_hasher();
    Ok(Self {
      length,
      received: 0,
      block_size,
      block_received: 0,
      hash,
      leaves: Vec::new(),
    })
  }

  pub fn received(&self) -> u64 {
    self.received
  }

  /// An oversized update fails before changing state, allowing the caller to retry.
  pub fn update(&mut self, mut bytes: &[u8]) -> Result<(), BuildError> {
    let count = u64::try_from(bytes.len()).map_err(|_| BuildError::LengthExceeded)?;
    if count > self.length - self.received {
      return Err(BuildError::LengthExceeded);
    }
    let complete = bytes.len() / self.block_size as usize
      + usize::from(
        bytes.len() % self.block_size as usize + self.block_received >= self.block_size as usize,
      );
    self
      .leaves
      .try_reserve(complete)
      .map_err(|_| BuildError::TooManyBlocks)?;
    self.received += count;
    while !bytes.is_empty() {
      let take = bytes
        .len()
        .min(self.block_size as usize - self.block_received);
      self.hash.update(&bytes[..take]);
      self.block_received += take;
      bytes = &bytes[take..];
      if self.block_received == self.block_size as usize {
        self.leaves.push(
          std::mem::replace(&mut self.hash, merkle::leaf_hasher())
            .finalize()
            .into(),
        );
        self.block_received = 0;
      }
    }
    Ok(())
  }

  pub fn finish(mut self) -> Result<Seed, BuildError> {
    if self.received != self.length {
      return Err(BuildError::LengthMismatch);
    }
    if self.block_received != 0 {
      self
        .leaves
        .try_reserve(1)
        .map_err(|_| BuildError::TooManyBlocks)?;
      self.leaves.push(self.hash.finalize().into());
    }
    let mut info = Info::new(self.length, merkle::root(&self.leaves));
    info.block_size = self.block_size;
    let mut seed = Seed::new(info);
    seed.leaf_hashes = Some(self.leaves);
    Ok(seed)
  }
}
