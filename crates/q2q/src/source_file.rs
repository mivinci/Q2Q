//! Immutable local source preparation and upload-time integrity checks.
use q2q_core::merkle::{self, Hash, Proof, ProofTree};
use q2q_core::seed::{DEFAULT_BLOCK_SIZE, Info, Seed};
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// A locally prepared source. Use `Client::publish_file` to announce and serve it.
pub struct PreparedFile {
  path: PathBuf,
  file: File,
  seed: Seed,
  tree: ProofTree,
}

impl PreparedFile {
  pub(crate) fn open(path: PathBuf, track_url: String) -> Result<Self, PrepareError> {
    let name = path
      .file_name()
      .and_then(|name| name.to_str())
      .ok_or(PrepareError::InvalidName)?
      .to_owned();
    let mut file = File::open(&path).map_err(PrepareError::Io)?;
    let before = file.metadata().map_err(PrepareError::Io)?;
    if !before.is_file() {
      return Err(PrepareError::NotRegularFile);
    }
    let length = before.len();
    let block_size = DEFAULT_BLOCK_SIZE as usize;
    let block_count = length.div_ceil(DEFAULT_BLOCK_SIZE as u64);
    let block_count = usize::try_from(block_count).map_err(|_| PrepareError::TooManyBlocks)?;
    let mut leaves = Vec::new();
    leaves
      .try_reserve_exact(block_count)
      .map_err(|_| PrepareError::TooManyBlocks)?;
    let mut buffer = vec![0u8; block_size];
    for index in 0..block_count {
      let start = index as u64 * DEFAULT_BLOCK_SIZE as u64;
      let bytes = (length - start).min(DEFAULT_BLOCK_SIZE as u64) as usize;
      file
        .read_exact(&mut buffer[..bytes])
        .map_err(PrepareError::Io)?;
      leaves.push(merkle::leaf_hash(&buffer[..bytes]));
    }
    let after = file.metadata().map_err(PrepareError::Io)?;
    if after.len() != length
      || matches!((before.modified(), after.modified()), (Ok(a), Ok(b)) if a != b)
    {
      return Err(PrepareError::SourceChanged);
    }

    let tree = ProofTree::new(&leaves);
    let mut info = Info::new(length, tree.root());
    info.name = Some(name);
    let mut seed = Seed::new(info);
    seed.track_list.push(track_url);
    seed.leaf_hashes = Some(leaves);
    Ok(Self {
      path,
      file,
      seed,
      tree,
    })
  }

  pub fn path(&self) -> &Path {
    &self.path
  }

  pub fn seed(&self) -> &Seed {
    &self.seed
  }

  pub fn info_hash(&self) -> Hash {
    self.seed.info_hash()
  }

  pub fn block_count(&self) -> usize {
    self.seed.info.leaf_count()
  }

  /// Return a proof for a block in this seed. The upload path must pair it
  /// with `read_verified_block`, which refuses data changed since preparation.
  pub fn proof(&self, index: usize) -> Result<Proof, PrepareError> {
    self
      .tree
      .proof(index)
      .ok_or(PrepareError::BlockOutOfRange(index))
  }

  /// Read a block from the same open file and compare it to its committed leaf.
  /// A changed file is never served as if it matched the original seed.
  pub fn read_verified_block(&mut self, index: usize) -> Result<Vec<u8>, PrepareError> {
    let leaves = self
      .seed
      .leaf_hashes
      .as_ref()
      .expect("prepared seed has leaves");
    let expected = leaves
      .get(index)
      .ok_or(PrepareError::BlockOutOfRange(index))?;
    let start = index as u64 * DEFAULT_BLOCK_SIZE as u64;
    let bytes = (self.seed.info.length - start).min(DEFAULT_BLOCK_SIZE as u64) as usize;
    let mut data = vec![0u8; bytes];
    self
      .file
      .seek(SeekFrom::Start(start))
      .map_err(PrepareError::Io)?;
    self.file.read_exact(&mut data).map_err(PrepareError::Io)?;
    if merkle::leaf_hash(&data) != *expected {
      return Err(PrepareError::SourceChanged);
    }
    Ok(data)
  }
}

#[derive(Debug)]
pub enum PrepareError {
  InvalidName,
  NotRegularFile,
  TooManyBlocks,
  BlockOutOfRange(usize),
  SourceChanged,
  Io(io::Error),
  Task(tokio::task::JoinError),
}

impl fmt::Display for PrepareError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::InvalidName => f.write_str("file name must be valid UTF-8"),
      Self::NotRegularFile => f.write_str("source must be a regular file"),
      Self::TooManyBlocks => f.write_str("file has too many blocks for this platform"),
      Self::BlockOutOfRange(i) => write!(f, "block out of range: {i}"),
      Self::SourceChanged => f.write_str("source file changed after preparation"),
      Self::Io(e) => write!(f, "source file I/O failed: {e}"),
      Self::Task(e) => write!(f, "file preparation task failed: {e}"),
    }
  }
}

impl std::error::Error for PrepareError {
  fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
    match self {
      Self::Io(e) => Some(e),
      Self::Task(e) => Some(e),
      _ => None,
    }
  }
}
