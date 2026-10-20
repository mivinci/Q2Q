//! Immutable local source preparation and upload-time integrity checks.
use q2q_primitives::merkle::{self, Hash, Proof, ProofTree};
use q2q_primitives::seed::{DEFAULT_BLOCK_SIZE, Seed};
use q2q_primitives::seed_builder::SeedBuilder;
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
#[cfg(not(any(unix, windows)))]
use std::{
  io::{Seek, SeekFrom},
  sync::Mutex,
};
#[cfg(test)]
use tokio::sync::OwnedSemaphorePermit;
use tokio::sync::{Semaphore, watch};

/// Current stage of publishing a local file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PublishPhase {
  /// Reading and hashing the source file.
  #[default]
  Preparing,
  /// Connecting to Track and announcing the prepared seed.
  Announcing,
}

/// Coalesced progress for preparing and announcing a publication.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PublishProgress {
  pub phase: PublishPhase,
  pub bytes_read: u64,
  pub total_bytes: u64,
}

/// A locally prepared source. Use `Client::publish_prepared` to announce and serve it.
/// Clones share the same opened file, immutable metadata, proof tree and read budget.
#[derive(Clone)]
pub struct PreparedFile {
  path: PathBuf,
  file: Arc<File>,
  seed: Arc<Seed>,
  tree: Arc<ProofTree>,
  modified: Option<SystemTime>,
  reads: Arc<Semaphore>,
  #[cfg(not(any(unix, windows)))]
  cursor: Arc<Mutex<()>>,
}

impl PreparedFile {
  /// Hash a local file without constructing a Client or contacting Track.
  /// This performs blocking I/O; async callers should use a blocking task.
  pub fn open(path: PathBuf, track_url: String) -> Result<Self, PrepareError> {
    Self::open_inner(path, track_url, None, None)
  }

  pub fn open_with_progress(
    path: PathBuf,
    track_url: String,
    progress: watch::Sender<PublishProgress>,
  ) -> Result<Self, PrepareError> {
    Self::open_inner(path, track_url, Some(progress), None)
  }

  /// Prepare an existing seed against local bytes without network access.
  pub fn from_seed(path: PathBuf, seed: Seed) -> Result<Self, PrepareError> {
    Self::open_inner(path, String::new(), None, Some(seed))
  }

  pub fn from_seed_with_progress(
    path: PathBuf,
    seed: Seed,
    progress: watch::Sender<PublishProgress>,
  ) -> Result<Self, PrepareError> {
    Self::open_inner(path, String::new(), Some(progress), Some(seed))
  }

  fn open_inner(
    path: PathBuf,
    track_url: String,
    progress: Option<watch::Sender<PublishProgress>>,
    expected: Option<Seed>,
  ) -> Result<Self, PrepareError> {
    if let Some(seed) = &expected {
      seed.validate().map_err(PrepareError::InvalidSeed)?;
    }
    let name = if expected.is_none() {
      Some(
        path
          .file_name()
          .and_then(|name| name.to_str())
          .ok_or(PrepareError::InvalidName)?
          .to_owned(),
      )
    } else {
      None
    };
    let mut file = File::open(&path).map_err(PrepareError::Io)?;
    let before = file.metadata().map_err(PrepareError::Io)?;
    if !before.is_file() {
      return Err(PrepareError::NotRegularFile);
    }
    let length = before.len();
    if expected
      .as_ref()
      .is_some_and(|seed| seed.info.length != length)
    {
      return Err(PrepareError::SeedMismatch);
    }
    let block_size = expected
      .as_ref()
      .map_or(DEFAULT_BLOCK_SIZE, |seed| seed.info.block_size);
    let block_count = length.div_ceil(block_size as u64);
    if block_count > q2q_primitives::peer_wire::MAX_BLOCKS as u64 {
      return Err(PrepareError::TooManyBlocks);
    }

    if let Some(progress) = &progress {
      progress.send_replace(PublishProgress {
        phase: PublishPhase::Preparing,
        bytes_read: 0,
        total_bytes: length,
      });
    }
    let mut builder =
      SeedBuilder::with_block_size(length, block_size).map_err(|_| PrepareError::TooManyBlocks)?;
    let block_size = block_size as usize;
    let mut buffer = vec![0u8; block_size];
    let mut bytes_read = 0;
    let mut last_progress = Instant::now();
    for index in 0..block_count {
      let start = index * block_size as u64;
      let bytes = (length - start).min(block_size as u64) as usize;
      file
        .read_exact(&mut buffer[..bytes])
        .map_err(PrepareError::Io)?;
      builder
        .update(&buffer[..bytes])
        .map_err(|_| PrepareError::TooManyBlocks)?;
      bytes_read += bytes as u64;
      if let Some(progress) = &progress
        && (last_progress.elapsed() >= Duration::from_millis(100) || index + 1 == block_count)
      {
        progress.send_replace(PublishProgress {
          phase: PublishPhase::Preparing,
          bytes_read,
          total_bytes: length,
        });
        last_progress = Instant::now();
      }
    }
    if block_count == 0
      && let Some(progress) = &progress
    {
      progress.send_replace(PublishProgress {
        phase: PublishPhase::Preparing,
        bytes_read: 0,
        total_bytes: 0,
      });
    }
    let after = file.metadata().map_err(PrepareError::Io)?;
    if after.len() != length
      || matches!((before.modified(), after.modified()), (Ok(a), Ok(b)) if a != b)
    {
      return Err(PrepareError::SourceChanged);
    }

    let mut seed = builder.finish().map_err(|_| PrepareError::TooManyBlocks)?;
    seed.info.name = name;
    if let Some(mut original) = expected {
      if original.info.root != seed.info.root {
        return Err(PrepareError::SeedMismatch);
      }
      original.leaf_hashes = seed.leaf_hashes;
      seed = original;
    }

    let tree = ProofTree::new(
      seed
        .leaf_hashes
        .as_ref()
        .expect("builder returns leaf hashes"),
    );
    if !track_url.is_empty() && !seed.track_list.contains(&track_url) {
      seed.track_list.insert(0, track_url);
    }
    Ok(Self {
      path,
      file: Arc::new(file),
      seed: Arc::new(seed),
      tree: Arc::new(tree),
      modified: after.modified().ok(),
      reads: Arc::new(Semaphore::new(4)),
      #[cfg(not(any(unix, windows)))]
      cursor: Arc::new(Mutex::new(())),
    })
  }

  pub(crate) fn with_track(mut self, track: String) -> Self {
    let seed = Arc::make_mut(&mut self.seed);
    let parsed = url::Url::parse(&track).ok();
    seed
      .track_list
      .retain(|value| value != &track && url::Url::parse(value).ok() != parsed);
    seed.track_list.insert(0, track);
    self
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
  /// This synchronous API does not acquire the SDK async read budget; callers
  /// creating their own threads must bound their own concurrency.
  pub fn read_verified_block(&self, index: usize) -> Result<Vec<u8>, PrepareError> {
    let leaves = self
      .seed
      .leaf_hashes
      .as_ref()
      .expect("prepared seed has leaves");
    let expected = leaves
      .get(index)
      .ok_or(PrepareError::BlockOutOfRange(index))?;
    let block_size = self.seed.info.block_size as u64;
    let start = index as u64 * block_size;
    let bytes = (self.seed.info.length - start).min(block_size) as usize;
    let mut data = vec![0u8; bytes];
    self.check_unchanged()?;
    self.read_exact_at(&mut data, start).map_err(|error| {
      if error.kind() == io::ErrorKind::UnexpectedEof {
        PrepareError::SourceChanged
      } else {
        PrepareError::Io(error)
      }
    })?;
    self.check_unchanged()?;
    if merkle::leaf_hash(&data) != *expected {
      return Err(PrepareError::SourceChanged);
    }
    Ok(data)
  }

  pub(crate) fn with_read_limit(mut self, reads: Arc<Semaphore>) -> Self {
    self.reads = reads;
    self
  }

  pub(crate) fn read_limit(&self) -> Arc<Semaphore> {
    self.reads.clone()
  }

  #[cfg(test)]
  pub(crate) async fn read_permit(&self) -> OwnedSemaphorePermit {
    self
      .reads
      .clone()
      .acquire_owned()
      .await
      .expect("source read gate is never closed")
  }

  fn check_unchanged(&self) -> Result<(), PrepareError> {
    self.source_revision().map(|_| ())
  }

  pub(crate) fn source_revision(
    &self,
  ) -> Result<crate::upload_source::SourceRevision, PrepareError> {
    let metadata = self.file.metadata().map_err(PrepareError::Io)?;
    if metadata.len() != self.seed.info.length
      || matches!((self.modified, metadata.modified()), (Some(before), Ok(after)) if before != after)
    {
      return Err(PrepareError::SourceChanged);
    }
    Ok(crate::upload_source::SourceRevision::from_metadata(
      &metadata,
    ))
  }

  #[cfg(any(unix, windows))]
  fn read_exact_at(&self, data: &mut [u8], offset: u64) -> io::Result<()> {
    // Every syscall gets its own absolute offset. Neither short reads nor
    // interleaved callers can move the next read to another block.
    read_exact_with(data, offset, |buffer, position| {
      #[cfg(unix)]
      {
        std::os::unix::fs::FileExt::read_at(&*self.file, buffer, position)
      }
      #[cfg(windows)]
      {
        std::os::windows::fs::FileExt::seek_read(&*self.file, buffer, position)
      }
    })
  }

  #[cfg(not(any(unix, windows)))]
  fn read_exact_at(&self, data: &mut [u8], offset: u64) -> io::Result<()> {
    // Conservative portability fallback; Unix/Windows never take this lock.
    let _cursor = self
      .cursor
      .lock()
      .unwrap_or_else(|error| error.into_inner());
    let mut file = &*self.file;
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(data)
  }
}

#[cfg(any(unix, windows))]
fn read_exact_with(
  mut data: &mut [u8],
  mut offset: u64,
  mut read: impl FnMut(&mut [u8], u64) -> io::Result<usize>,
) -> io::Result<()> {
  while !data.is_empty() {
    match read(data, offset) {
      Ok(0) => {
        return Err(io::Error::new(
          io::ErrorKind::UnexpectedEof,
          "source block ended early",
        ));
      }
      Ok(bytes) => {
        offset += bytes as u64;
        data = &mut data[bytes..];
      }
      Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
      Err(error) => return Err(error),
    }
  }
  Ok(())
}

#[derive(Debug)]
pub enum PrepareError {
  InvalidName,
  NotRegularFile,
  TooManyBlocks,
  BlockOutOfRange(usize),
  SourceChanged,
  SeedMismatch,
  InvalidSeed(q2q_primitives::seed::SeedError),
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
      Self::SeedMismatch => {
        f.write_str("source length or Merkle root does not match the supplied seed")
      }
      Self::InvalidSeed(error) => write!(f, "invalid source seed: {error}"),
      Self::SourceChanged => f.write_str("source file changed after preparation"),
      Self::Io(e) => write!(f, "source file I/O failed: {e}"),
      Self::Task(e) => write!(f, "file preparation task failed: {e}"),
    }
  }
}

impl std::error::Error for PrepareError {
  fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
    match self {
      Self::InvalidSeed(e) => Some(e),
      Self::Io(e) => Some(e),
      Self::Task(e) => Some(e),
      _ => None,
    }
  }
}

#[cfg(test)]
#[path = "../tests/source_file.rs"]
mod tests;
