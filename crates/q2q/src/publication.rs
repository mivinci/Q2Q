//! Local source preparation and a managed publication lifecycle.

use crate::control::{ControlError, ControlSession};
use crate::transfer::TransferError;
use q2q_core::merkle::{self, Hash, Proof};
use q2q_core::seed::{DEFAULT_BLOCK_SIZE, Info, Seed};
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{
  Arc, Mutex,
  atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;

pub(crate) struct ActivePublication(Arc<AtomicBool>);
impl ActivePublication {
  pub(crate) fn acquire(active: Arc<AtomicBool>) -> Result<Self, TransferError> {
    active
      .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
      .map_err(|_| TransferError::AlreadyPublishing)?;
    Ok(Self(active))
  }
}
impl Drop for ActivePublication {
  fn drop(&mut self) {
    self.0.store(false, Ordering::Release);
  }
}

/// A live source. Dropping the handle stops serving and asynchronously withdraws
/// the Track holding; `close` waits for withdrawal and reports any failure.
pub struct Publication {
  seed: Seed,
  stop: watch::Sender<bool>,
  task: Option<JoinHandle<Result<(), TransferError>>>,
}

impl Publication {
  pub(crate) async fn start(
    prepared: PreparedFile,
    session: Arc<ControlSession>,
    active: ActivePublication,
  ) -> Result<Self, TransferError> {
    let seed = prepared.seed().clone();
    let hash = seed.info_hash();
    let source = Arc::new(Mutex::new(prepared));
    let (stop, mut stopped) = watch::channel(false);
    let (ready, started) = oneshot::channel();
    let task = tokio::spawn(async move {
      let _active = active;
      let expires = match session.update_files(&[hash], &[]).await {
        Ok(expires) => expires,
        Err(error) => {
          let _ = session.update_files(&[], &[hash]).await;
          let _ = ready.send(Err(TransferError::Control(error)));
          return Ok(());
        }
      };
      if ready.send(Ok(())).is_err() {
        return session
          .update_files(&[], &[hash])
          .await
          .map(|_| ())
          .map_err(TransferError::Control);
      }
      let serving = async {
        loop {
          let peer = match session.accept_peer(hash).await {
            Ok(peer) => peer,
            Err(_) => {
              // One failed or abandoned handshake does not withdraw a healthy
              // publication. Heartbeat detects a dead standing Binding.
              tokio::time::sleep(Duration::from_millis(100)).await;
              continue;
            }
          };
          match peer.serve_file(source.clone()).await {
            Ok(())
            | Err(
              TransferError::Closed
              | TransferError::Timeout
              | TransferError::Protocol(_)
              | TransferError::Wire(_),
            ) => {}
            Err(error) => return Err(error),
          }
        }
      };
      let renewing = async {
        let mut deadline = expires;
        loop {
          let delay = deadline.saturating_sub(q2q_track::state::now()).max(2) / 2;
          tokio::time::sleep(Duration::from_secs(delay)).await;
          match session.update_files(&[hash], &[]).await {
            Ok(expires) => deadline = expires,
            Err(error) => break Err::<(), TransferError>(error.into()),
          }
        }
      };
      let result = tokio::select! {
        result = serving => result,
        result = renewing => result,
        error = keep_online(&session) => Err(TransferError::Control(error)),
        _ = stopped.changed() => Ok(()),
      };
      let withdrawn = session
        .update_files(&[], &[hash])
        .await
        .map(|_| ())
        .map_err(TransferError::Control);
      result.and(withdrawn)
    });
    let publication = Self {
      seed,
      stop,
      task: Some(task),
    };
    match started.await {
      Ok(Ok(())) => Ok(publication),
      Ok(Err(error)) => {
        let _ = publication.close().await;
        Err(error)
      }
      Err(_) => {
        publication.close().await?;
        Err(TransferError::Protocol(
          "publication stopped before announcing".into(),
        ))
      }
    }
  }

  pub fn seed(&self) -> &Seed {
    &self.seed
  }
  pub fn info_hash(&self) -> Hash {
    self.seed.info_hash()
  }
  pub fn is_finished(&self) -> bool {
    self.task.as_ref().is_none_or(|task| task.is_finished())
  }

  pub async fn close(mut self) -> Result<(), TransferError> {
    let _ = self.stop.send(true);
    let result = self.task.as_mut().expect("publication task exists").await?;
    self.task.take();
    result
  }
}

impl Drop for Publication {
  fn drop(&mut self) {
    let _ = self.stop.send(true);
  }
}

pub(crate) async fn keep_online(session: &ControlSession) -> ControlError {
  loop {
    tokio::time::sleep(Duration::from_secs(
      u64::from(session.heartbeat_interval()).max(1),
    ))
    .await;
    if let Err(error) = session.heartbeat().await {
      return error;
    }
  }
}

/// A locally prepared source. Use `Client::publish_file` to announce and serve it.
pub struct PreparedFile {
  path: PathBuf,
  file: File,
  seed: Seed,
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

    let mut info = Info::new(length, merkle::root(&leaves));
    info.name = Some(name);
    let mut seed = Seed::new(info);
    seed.track_list.push(track_url);
    seed.leaf_hashes = Some(leaves);
    Ok(Self { path, file, seed })
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
    let leaves = self
      .seed
      .leaf_hashes
      .as_ref()
      .expect("prepared seed has leaves");
    if index >= leaves.len() {
      return Err(PrepareError::BlockOutOfRange(index));
    }
    Ok(merkle::prove(leaves, index))
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
