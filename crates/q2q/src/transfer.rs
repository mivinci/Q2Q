//! Verified block transfer with bounded buffering and selective piece retry.

use crate::control::ControlError;
use crate::download_store::DownloadStore;
use crate::peer::PeerSession;
pub use crate::progress::{TransferProgress, TransferState};
use crate::publication::{PrepareError, PreparedFile};
use crate::scheduler::WorkQueue;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use bytes::BytesMut;
use q2q_core::block::PIECE_SIZE;
use q2q_core::file::StoreError;
use q2q_core::merkle::{self, Proof};
use q2q_core::peer_wire::{Control, MAX_BLOCKS, Piece, WireError};
use q2q_core::seed::Seed;
use std::collections::HashSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use webrtc::data_channel::{DataChannel, DataChannelEvent};

const IDLE: Duration = Duration::from_secs(30);
const BLOCK_DEADLINE: Duration = Duration::from_secs(20);
const RETRY_DELAY: Duration = Duration::from_millis(500);

#[derive(Debug)]
pub enum TransferError {
  Control(ControlError),
  Prepare(PrepareError),
  Store(StoreError),
  Wire(WireError),
  Io(std::io::Error),
  Task(tokio::task::JoinError),
  Protocol(String),
  Checkpoint(String),
  Closed,
  Timeout,
  Rejected(u32),
  NoPeers,
  AlreadyPublishing,
}

impl std::fmt::Display for TransferError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      Self::Control(e) => e.fmt(f),
      Self::Prepare(e) => e.fmt(f),
      Self::Store(e) => e.fmt(f),
      Self::Wire(e) => e.fmt(f),
      Self::Io(e) => e.fmt(f),
      Self::Task(e) => e.fmt(f),
      Self::Protocol(e) => write!(f, "peer transfer protocol: {e}"),
      Self::Checkpoint(e) => write!(f, "download checkpoint: {e}"),
      Self::Closed => f.write_str("peer transfer channel closed"),
      Self::Timeout => f.write_str("peer transfer timed out"),
      Self::Rejected(ms) => write!(f, "peer rejected request; retry after {ms} ms"),
      Self::NoPeers => f.write_str("no complete peer is available"),
      Self::AlreadyPublishing => f.write_str("this client already has an active publication"),
    }
  }
}
impl std::error::Error for TransferError {}
macro_rules! convert {
  ($ty:ty, $variant:ident) => {
    impl From<$ty> for TransferError {
      fn from(e: $ty) -> Self {
        Self::$variant(e)
      }
    }
  };
}
convert!(ControlError, Control);
convert!(PrepareError, Prepare);
convert!(StoreError, Store);
convert!(WireError, Wire);
convert!(std::io::Error, Io);
convert!(tokio::task::JoinError, Task);

/// A download task. Dropping or cancelling it stops the transfer; an incomplete
/// destination remains unverified and `wait` never reports it as complete.
pub struct Transfer {
  pub(crate) task: Option<JoinHandle<Result<PathBuf, TransferError>>>,
  pub(crate) progress: watch::Sender<TransferProgress>,
}
impl Transfer {
  pub(crate) fn start(
    progress: watch::Sender<TransferProgress>,
    operation: impl Future<Output = Result<PathBuf, TransferError>> + Send + 'static,
  ) -> Self {
    let updates = progress.clone();
    let task = tokio::spawn(async move {
      let result = operation.await;
      crate::progress::finish(
        &updates,
        if result.is_ok() {
          TransferState::Completed
        } else {
          TransferState::Failed
        },
      );
      result
    });
    Self {
      task: Some(task),
      progress,
    }
  }

  /// Latest snapshot; byte counts include only verified, persisted blocks.
  pub fn progress(&self) -> TransferProgress {
    *self.progress.borrow()
  }

  /// Coalesced snapshots. Slow consumers observe the latest value without
  /// buffering every block event. A terminal state is published before wait returns.
  pub fn subscribe_progress(&self) -> watch::Receiver<TransferProgress> {
    self.progress.subscribe()
  }

  pub async fn wait(mut self) -> Result<PathBuf, TransferError> {
    let result = self.task.as_mut().expect("transfer task exists").await;
    self.task.take();
    match result {
      Ok(result) => result,
      Err(error) => {
        crate::progress::finish(&self.progress, TransferState::Failed);
        Err(error.into())
      }
    }
  }
  pub fn cancel(self) {
    drop(self);
  }
}
impl Drop for Transfer {
  fn drop(&mut self) {
    if let Some(task) = &self.task {
      task.abort();
      crate::progress::finish(&self.progress, TransferState::Cancelled);
    }
  }
}

pub(crate) async fn create_destination(
  seed: &Seed,
  path: &Path,
) -> Result<Arc<Mutex<DownloadStore>>, TransferError> {
  open_destination(seed, path, false).await
}

pub(crate) async fn open_destination(
  seed: &Seed,
  path: &Path,
  resume: bool,
) -> Result<Arc<Mutex<DownloadStore>>, TransferError> {
  check_seed(seed)?;
  let seed = seed.clone();
  let path = path.to_path_buf();
  tokio::task::spawn_blocking(move || {
    let store = if resume {
      DownloadStore::resume(&seed, &path)?
    } else {
      DownloadStore::create(&seed, &path)?
    };
    Ok(Arc::new(Mutex::new(store)))
  })
  .await?
}

pub(crate) fn destination_complete(store: &Mutex<DownloadStore>) -> Result<bool, TransferError> {
  let store = store
    .lock()
    .map_err(|_| protocol("destination lock poisoned"))?;
  for index in 0..store.block_count() {
    if store.state(index)? != q2q_core::block::BlockState::Verified {
      return Ok(false);
    }
  }
  Ok(true)
}

pub(crate) async fn sync_destination(path: &Path) -> Result<(), TransferError> {
  let path = path.to_path_buf();
  tokio::task::spawn_blocking(move || {
    std::fs::OpenOptions::new()
      .write(true)
      .open(path)?
      .sync_all()
  })
  .await??;
  Ok(())
}

impl TransferError {
  pub(crate) fn peer_retryable(&self) -> bool {
    matches!(
      self,
      Self::Control(_)
        | Self::Wire(_)
        | Self::Protocol(_)
        | Self::Closed
        | Self::Timeout
        | Self::Rejected(_)
        | Self::Store(StoreError::RootMismatch)
    )
  }
}

impl PeerSession {
  /// Serve a prepared source until the downloader confirms the end of this session.
  pub async fn serve_file(self, source: Arc<Mutex<PreparedFile>>) -> Result<(), TransferError> {
    let seed = source
      .lock()
      .map_err(|_| protocol("source lock poisoned"))?
      .seed()
      .clone();
    check_seed(&seed)?;
    if &seed.info_hash() != self.info_hash() {
      return Err(protocol("source belongs to another session"));
    }
    let mut full = vec![0xff; seed.info.leaf_count().div_ceil(8)];
    if let Some(last) = full.last_mut() {
      let remainder = seed.info.leaf_count() % 8;
      if remainder != 0 {
        *last &= 0xff << (8 - remainder);
      }
    }
    send(
      &*self.control,
      Control::Bitfield {
        bits: B64.encode(full),
      },
    )
    .await?;
    receive_bitfield(&*self.control, seed.info.leaf_count()).await?;
    let mut pending: Option<(usize, Vec<u8>, Proof)> = None;
    loop {
      let control = tokio::time::timeout(IDLE, receive(&*self.control))
        .await
        .map_err(|_| TransferError::Timeout)??;
      match control {
        Control::Request { block_index, count } => {
          let index =
            usize::try_from(block_index).map_err(|_| protocol("block index overflows platform"))?;
          if count != 1 || index >= seed.info.leaf_count() || pending.is_some() {
            send(
              &*self.control,
              Control::Reject {
                block_index,
                count,
                retry_after: 100,
              },
            )
            .await?;
            continue;
          }
          let source = source.clone();
          let (bytes, proof) = tokio::task::spawn_blocking(move || {
            let mut source = source
              .lock()
              .map_err(|_| protocol("source lock poisoned"))?;
            Ok::<_, TransferError>((source.read_verified_block(index)?, source.proof(index)?))
          })
          .await??;
          send(
            &*self.control,
            Control::Proof {
              block_index,
              count: 1,
              proof: proof.clone(),
            },
          )
          .await?;
          tokio::time::timeout(
            BLOCK_DEADLINE,
            send_pieces(&*self.data, block_index, &bytes, &[(0, bytes.len() as u32)]),
          )
          .await
          .map_err(|_| TransferError::Timeout)??;
          pending = Some((index, bytes, proof));
        }
        Control::PieceNack {
          block_index,
          piece_offset,
          length,
        } => {
          let Some((index, bytes, _)) = &pending else {
            return Err(protocol("piece retry has no active block"));
          };
          if *index as u64 != block_index {
            return Err(protocol("piece retry names another block"));
          }
          let ranges: Vec<_> = piece_offset.into_iter().zip(length).collect();
          let mut previous_end = 0;
          for &(offset, len) in &ranges {
            let end = u64::from(offset) + u64::from(len);
            if len == 0
              || u64::from(offset) < previous_end
              || !(offset as usize).is_multiple_of(PIECE_SIZE)
              || end > bytes.len() as u64
              || (end != bytes.len() as u64 && !(len as usize).is_multiple_of(PIECE_SIZE))
            {
              return Err(protocol("invalid piece retry range"));
            }
            previous_end = end;
          }
          tokio::time::timeout(
            BLOCK_DEADLINE,
            send_pieces(&*self.data, block_index, bytes, &ranges),
          )
          .await
          .map_err(|_| TransferError::Timeout)??;
        }
        Control::RequestProof {
          block_index,
          count: 1,
        } => {
          if let Some((index, _, proof)) = &pending {
            if *index as u64 != block_index {
              return Err(protocol("proof names another block"));
            }
            send(
              &*self.control,
              Control::Proof {
                block_index,
                count: 1,
                proof: proof.clone(),
              },
            )
            .await?;
          }
        }
        Control::BlockDone { block_index } => {
          if pending
            .as_ref()
            .is_none_or(|(index, _, _)| *index as u64 != block_index)
          {
            return Err(protocol("unexpected block acknowledgement"));
          }
          pending = None;
        }
        Control::Have { .. } => {}
        Control::Cancel { block_index, count } => {
          // Acknowledge the ordered end marker before either side closes RTC.
          // It also ends sessions that supplied only a subset after failover.
          send(&*self.control, Control::Cancel { block_index, count }).await?;
          return match tokio::time::timeout(IDLE, receive(&*self.control)).await {
            Ok(Err(TransferError::Closed)) => Ok(()),
            Ok(Err(error)) => Err(error),
            Ok(Ok(_)) => Err(protocol("unexpected message after transfer ended")),
            Err(_) => Err(TransferError::Timeout),
          };
        }
        _ => return Err(protocol("unexpected upload control message")),
      }
    }
  }

  /// Download to a new file. Existing destinations are never truncated.
  pub async fn download_file(
    self,
    seed: Seed,
    path: impl AsRef<Path>,
  ) -> Result<PathBuf, TransferError> {
    check_seed(&seed)?;
    if &seed.info_hash() != self.info_hash() {
      return Err(protocol("seed belongs to another session"));
    }
    let path = path.as_ref().to_path_buf();
    let store = create_destination(&seed, &path).await?;
    self.download_into(&seed, store.clone()).await?;
    sync_destination(&path).await?;
    Ok(path)
  }

  /// Continue a transfer using blocks verified in memory or revalidated from
  /// root-bound checkpoint proofs. Persisted state flags are never trusted.
  pub(crate) async fn download_into(
    self,
    seed: &Seed,
    store: Arc<Mutex<DownloadStore>>,
  ) -> Result<(), TransferError> {
    let bits = store
      .lock()
      .map_err(|_| protocol("destination lock poisoned"))?
      .verified_bitfield();
    let progress = crate::progress::channel(seed, &bits);
    let queue = WorkQueue::new(seed, &bits, progress);
    self.download_scheduled(seed, store, queue).await
  }

  pub(crate) async fn download_scheduled(
    self,
    seed: &Seed,
    store: Arc<Mutex<DownloadStore>>,
    queue: Arc<WorkQueue>,
  ) -> Result<(), TransferError> {
    if &seed.info_hash() != self.info_hash() {
      return Err(protocol("seed belongs to another session"));
    }
    let count = seed.info.leaf_count();
    let bits = queue.bitfield();
    send(
      &*self.control,
      Control::Bitfield {
        bits: B64.encode(bits),
      },
    )
    .await?;
    let available = receive_bitfield(&*self.control, count).await?;
    if (0..count).any(|i| available[i / 8] & (1 << (7 - i % 8)) == 0) {
      return Err(protocol("peer does not hold the complete file"));
    }
    let _active = crate::progress::ActivePeer::new(queue.progress.clone());
    let mut completed = HashSet::new();
    while let Some(lease) = queue.claim().await {
      let index = lease.index;
      let length =
        crate::progress::block_len(seed.info.length, seed.info.block_size, index) as usize;
      send(
        &*self.control,
        Control::Request {
          block_index: index as u64,
          count: 1,
        },
      )
      .await?;
      let (bytes, proof) = self.receive_block(index, length, &completed).await?;
      let root = seed.info.root;
      // Reject untrusted data before it can overwrite the destination.
      if !merkle::verify(&bytes, index, count, &proof, &root) {
        return Err(TransferError::Store(StoreError::RootMismatch));
      }
      let destination = store.clone();
      tokio::task::spawn_blocking(move || {
        let mut store = destination
          .lock()
          .map_err(|_| protocol("destination lock poisoned"))?;
        store.commit(index, &bytes, proof, &root)?;
        lease.commit();
        Ok::<_, TransferError>(())
      })
      .await??;
      completed.insert(index as u64);
      send(
        &*self.control,
        Control::Have {
          block_index: index as u64,
        },
      )
      .await?;
      send(
        &*self.control,
        Control::BlockDone {
          block_index: index as u64,
        },
      )
      .await?;
    }
    send(
      &*self.control,
      Control::Cancel {
        block_index: 0,
        count: 0,
      },
    )
    .await?;
    tokio::time::timeout(IDLE, async {
      loop {
        match receive(&*self.control).await? {
          Control::Cancel {
            block_index: 0,
            count: 0,
          } => return Ok(()),
          // A proof retry may already have been queued before BlockDone.
          Control::Proof { block_index, .. } if completed.contains(&block_index) => {}
          _ => return Err(protocol("expected transfer end acknowledgement")),
        }
      }
    })
    .await
    .map_err(|_| TransferError::Timeout)?
  }

  async fn receive_block(
    &self,
    index: usize,
    length: usize,
    completed: &HashSet<u64>,
  ) -> Result<(Vec<u8>, Proof), TransferError> {
    let mut assembly = Assembly::new(length);
    let mut proof = None;
    let mut retries = 0;
    let idle = tokio::time::sleep(RETRY_DELAY);
    tokio::pin!(idle);
    tokio::time::timeout(BLOCK_DEADLINE, async {
      loop {
        tokio::select! {
          control = receive(&*self.control) => match control? {
            Control::Proof { block_index, count: 1, proof: received } if block_index == index as u64 => proof = Some(received),
            Control::Proof { block_index, .. } if completed.contains(&block_index) => {},
            Control::Reject { retry_after, .. } => return Err(TransferError::Rejected(retry_after)),
            Control::Have { .. } => {},
            _ => return Err(protocol("unexpected download control message")),
          },
          event = self.data.poll() => match event {
            Some(DataChannelEvent::OnMessage(message)) => {
              if message.is_string { return Err(protocol("piece must be binary")); }
              let piece = Piece::decode(&message.data)?;
              if completed.contains(&piece.block_index) { continue; }
              if piece.block_index != index as u64 { return Err(protocol("unsolicited block")); }
              if assembly.insert(piece.piece_offset, piece.data)? {
                idle.as_mut().reset(tokio::time::Instant::now() + RETRY_DELAY);
              }
            },
            Some(DataChannelEvent::OnClose) | None => return Err(TransferError::Closed),
            _ => {},
          },
          _ = &mut idle => {
            if retries == 3 { return Err(TransferError::Timeout); }
            retries += 1;
            if proof.is_none() { send(&*self.control, Control::RequestProof { block_index: index as u64, count: 1 }).await?; }
            for ranges in assembly.missing().chunks(64) {
              send(&*self.control, Control::PieceNack {
                block_index: index as u64,
                piece_offset: ranges.iter().map(|(offset, _)| *offset).collect(),
                length: ranges.iter().map(|(_, length)| *length).collect(),
              }).await?;
            }
            idle.as_mut().reset(tokio::time::Instant::now() + RETRY_DELAY);
          },
        }
        if assembly.complete() && let Some(proof) = proof.take() { return Ok((assembly.bytes, proof)); }
      }
    }).await.map_err(|_| TransferError::Timeout)?
  }
}

struct Assembly {
  bytes: Vec<u8>,
  present: Vec<bool>,
  received: usize,
}
impl Assembly {
  fn new(length: usize) -> Self {
    Self {
      bytes: vec![0; length],
      present: vec![false; length.div_ceil(PIECE_SIZE)],
      received: 0,
    }
  }
  fn insert(&mut self, offset: u32, data: &[u8]) -> Result<bool, TransferError> {
    let offset = offset as usize;
    if !offset.is_multiple_of(PIECE_SIZE)
      || offset >= self.bytes.len()
      || data.len() != (self.bytes.len() - offset).min(PIECE_SIZE)
    {
      return Err(protocol("piece length or offset disagrees with seed"));
    }
    let slot = offset / PIECE_SIZE;
    if self.present[slot] {
      if self.bytes[offset..offset + data.len()] != *data {
        return Err(protocol("conflicting duplicate piece"));
      }
      return Ok(false);
    }
    self.bytes[offset..offset + data.len()].copy_from_slice(data);
    self.present[slot] = true;
    self.received += 1;
    Ok(true)
  }
  fn complete(&self) -> bool {
    self.received == self.present.len()
  }
  fn missing(&self) -> Vec<(u32, u32)> {
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    for (index, present) in self.present.iter().enumerate() {
      if *present {
        continue;
      }
      let offset = index * PIECE_SIZE;
      let len = (self.bytes.len() - offset).min(PIECE_SIZE) as u32;
      if let Some((start, length)) = ranges.last_mut()
        && *start + *length == offset as u32
      {
        *length += len;
      } else {
        ranges.push((offset as u32, len));
      }
    }
    ranges
  }
}

pub(crate) fn check_seed(seed: &Seed) -> Result<(), TransferError> {
  seed.validate().map_err(|e| protocol(e.to_string()))?;
  if seed.info.length.div_ceil(u64::from(seed.info.block_size)) > MAX_BLOCKS as u64 {
    return Err(protocol("single-peer bitfield limit exceeded"));
  }
  if seed.info.length == 0 && seed.info.root != merkle::EMPTY_ROOT {
    return Err(protocol("empty file has a nonempty Merkle root"));
  }
  Ok(())
}

async fn send(channel: &dyn DataChannel, message: Control) -> Result<(), TransferError> {
  tokio::time::timeout(IDLE, channel.send_text(&message.encode()?))
    .await
    .map_err(|_| TransferError::Timeout)?
    .map_err(|e| protocol(e.to_string()))
}

async fn receive(channel: &dyn DataChannel) -> Result<Control, TransferError> {
  loop {
    match channel.poll().await {
      Some(DataChannelEvent::OnMessage(message)) => {
        if !message.is_string {
          return Err(protocol("control message must be text"));
        }
        return Control::decode(&message.data).map_err(TransferError::Wire);
      }
      Some(DataChannelEvent::OnClose) | None => return Err(TransferError::Closed),
      _ => {}
    }
  }
}

async fn receive_bitfield(
  channel: &dyn DataChannel,
  count: usize,
) -> Result<Vec<u8>, TransferError> {
  let Control::Bitfield { bits } = tokio::time::timeout(IDLE, receive(channel))
    .await
    .map_err(|_| TransferError::Timeout)??
  else {
    return Err(protocol("expected initial bitfield"));
  };
  let bits = B64
    .decode(bits)
    .map_err(|_| protocol("bitfield is not base64"))?;
  if bits.len() != count.div_ceil(8)
    || (!count.is_multiple_of(8)
      && bits
        .last()
        .is_some_and(|byte| *byte & ((1 << (8 - count % 8)) - 1) != 0))
  {
    return Err(protocol("bitfield length or padding is invalid"));
  }
  Ok(bits)
}

async fn send_pieces(
  channel: &dyn DataChannel,
  index: u64,
  bytes: &[u8],
  ranges: &[(u32, u32)],
) -> Result<(), TransferError> {
  for &(offset, length) in ranges {
    for (piece, data) in bytes[offset as usize..(offset + length) as usize]
      .chunks(PIECE_SIZE)
      .enumerate()
    {
      let encoded = Piece {
        block_index: index,
        piece_offset: offset + (piece * PIECE_SIZE) as u32,
        data,
      }
      .encode()?;
      channel
        .send(BytesMut::from(encoded.as_slice()))
        .await
        .map_err(|e| protocol(e.to_string()))?;
      if piece % 16 == 0 {
        tokio::task::yield_now().await;
      }
    }
  }
  Ok(())
}

fn protocol(detail: impl Into<String>) -> TransferError {
  TransferError::Protocol(detail.into())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[tokio::test]
  async fn cancelling_wait_does_not_detach_a_background_download() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Finished(Arc<AtomicBool>);
    impl Drop for Finished {
      fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
      }
    }
    let finished = Arc::new(AtomicBool::new(false));
    let flag = finished.clone();
    let (ready, started) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
      let _finished = Finished(flag);
      let _ = ready.send(());
      std::future::pending::<Result<PathBuf, TransferError>>().await
    });
    started.await.unwrap();
    let prepared_dir = tempfile::tempdir().unwrap();
    let prepared = source(prepared_dir.path(), b"test");
    let transfer = Transfer {
      task: Some(task),
      progress: crate::progress::channel(prepared.seed(), &[]),
    };
    assert!(
      tokio::time::timeout(Duration::from_millis(10), transfer.wait())
        .await
        .is_err()
    );
    tokio::time::timeout(Duration::from_secs(1), async {
      while !finished.load(Ordering::Acquire) {
        tokio::task::yield_now().await;
      }
    })
    .await
    .expect("cancelled download remained detached");
  }

  #[test]
  fn reordered_and_missing_pieces_remain_bounded_and_conflicting_duplicates_fail() {
    let mut block = Assembly::new(2051);
    block.insert(2048, &[3; 3]).unwrap();
    block.insert(0, &[1; 1024]).unwrap();
    assert_eq!(block.missing(), vec![(1024, 1024)]);
    assert!(!block.complete());
    assert!(!block.insert(0, &[1; 1024]).unwrap());
    assert!(block.insert(0, &[9; 1024]).is_err());
    assert!(block.insert(1025, &[2; 1024]).is_err());
    assert!(block.insert(2048, &[3; 4]).is_err());
    block.insert(1024, &[2; 1024]).unwrap();
    assert!(block.complete());
  }

  fn source(directory: &Path, bytes: &[u8]) -> PreparedFile {
    let path = directory.join("source.bin");
    std::fs::write(&path, bytes).unwrap();
    PreparedFile::open(path, "http://localhost".into()).unwrap()
  }

  #[tokio::test]
  async fn full_file_uses_proofs_and_preserves_an_existing_destination() {
    let directory = tempfile::tempdir().unwrap();
    let bytes: Vec<_> = (0..(2 * 262144 + 19)).map(|i| (i % 251) as u8).collect();
    let prepared = source(directory.path(), &bytes);
    let mut seed = prepared.seed().clone();
    seed.leaf_hashes = None;
    let destination = directory.path().join("download.bin");
    let (download, upload) = crate::peer::test_pair(seed.info_hash()).await;
    let (received, served) = tokio::join!(
      download.download_file(seed.clone(), &destination),
      upload.serve_file(Arc::new(Mutex::new(prepared)))
    );
    received.unwrap();
    served.unwrap();
    assert_eq!(std::fs::read(&destination).unwrap(), bytes);

    let prepared = PreparedFile::open(
      directory.path().join("source.bin"),
      "http://localhost".into(),
    )
    .unwrap();
    let (download, upload) = crate::peer::test_pair(seed.info_hash()).await;
    let upload = async {
      send(
        &*upload.control,
        Control::Bitfield {
          bits: B64.encode([0xe0]),
        },
      )
      .await
      .unwrap();
      upload
    };
    let (received, _upload) = tokio::join!(download.download_file(seed, &destination), upload);
    assert!(
      matches!(received, Err(TransferError::Io(ref e)) if e.kind() == std::io::ErrorKind::AlreadyExists)
    );
    assert_eq!(std::fs::read(&destination).unwrap(), bytes);
    drop(prepared);
  }

  #[tokio::test]
  async fn switching_peers_keeps_verified_blocks_and_finishes_a_subset_session() {
    let directory = tempfile::tempdir().unwrap();
    let bytes = vec![73u8; 262144 + 19];
    let prepared = source(directory.path(), &bytes);
    let seed = prepared.seed().clone();
    let path = directory.path().join("download.bin");
    let store = create_destination(&seed, &path).await.unwrap();
    {
      let mut destination = store.lock().unwrap();
      for (piece, data) in bytes[..262144].chunks(PIECE_SIZE).enumerate() {
        destination.write_piece(0, piece, data).unwrap();
      }
      destination
        .verify(0, Some(&prepared.proof(0).unwrap()), &seed.info.root)
        .unwrap();
    }
    let (download, upload) = crate::peer::test_pair(seed.info_hash()).await;
    let (received, served) = tokio::time::timeout(Duration::from_secs(10), async {
      tokio::join!(
        download.download_into(&seed, store.clone()),
        upload.serve_file(Arc::new(Mutex::new(prepared)))
      )
    })
    .await
    .unwrap();
    received.unwrap();
    served.unwrap();
    assert!(destination_complete(&store).unwrap());
    assert_eq!(std::fs::read(path).unwrap(), bytes);
  }

  #[tokio::test]
  async fn dropped_piece_is_requested_again_and_corrupt_data_never_completes() {
    for corrupt in [false, true] {
      let directory = tempfile::tempdir().unwrap();
      let bytes = vec![42u8; 3077];
      let prepared = source(directory.path(), &bytes);
      let mut seed = prepared.seed().clone();
      seed.leaf_hashes = None;
      let proof = prepared.proof(0).unwrap();
      let (download, upload) = crate::peer::test_pair(seed.info_hash()).await;
      let upload = async {
        send(
          &*upload.control,
          Control::Bitfield {
            bits: B64.encode([0x80]),
          },
        )
        .await
        .unwrap();
        receive_bitfield(&*upload.control, 1).await.unwrap();
        assert!(matches!(
          receive(&*upload.control).await.unwrap(),
          Control::Request {
            block_index: 0,
            count: 1
          }
        ));
        send(
          &*upload.control,
          Control::Proof {
            block_index: 0,
            count: 1,
            proof,
          },
        )
        .await
        .unwrap();
        let mut bytes = bytes.clone();
        if corrupt {
          bytes[0] ^= 1;
        }
        send_pieces(&*upload.data, 0, &bytes, &[(0, 1024), (2048, 1029)])
          .await
          .unwrap();
        match tokio::time::timeout(Duration::from_secs(5), receive(&*upload.control))
          .await
          .unwrap()
          .unwrap()
        {
          Control::PieceNack {
            block_index: 0,
            piece_offset,
            length,
          } => {
            assert_eq!(piece_offset, vec![1024]);
            assert_eq!(length, vec![1024]);
            send_pieces(&*upload.data, 0, &bytes, &[(1024, 1024)])
              .await
              .unwrap();
          }
          message => panic!("expected selective retry, got {message:?}"),
        }
        if corrupt {
          assert!(matches!(
            receive(&*upload.control).await,
            Err(TransferError::Closed)
          ));
        } else {
          assert!(matches!(
            receive(&*upload.control).await.unwrap(),
            Control::Have { block_index: 0 }
          ));
          assert!(matches!(
            receive(&*upload.control).await.unwrap(),
            Control::BlockDone { block_index: 0 }
          ));
          let end = receive(&*upload.control).await.unwrap();
          assert!(matches!(
            end,
            Control::Cancel {
              block_index: 0,
              count: 0
            }
          ));
          send(&*upload.control, end).await.unwrap();
          assert!(matches!(
            receive(&*upload.control).await,
            Err(TransferError::Closed)
          ));
        }
      };
      let destination = directory.path().join("download.bin");
      let (received, ()) = tokio::join!(download.download_file(seed, &destination), upload);
      if corrupt {
        assert!(matches!(
          received,
          Err(TransferError::Store(StoreError::RootMismatch))
        ));
      } else {
        received.unwrap();
        assert_eq!(std::fs::read(destination).unwrap(), bytes);
      }
    }
  }

  // Real WebRTC peers with observable requests and an explicit end handshake.
  async fn observed_source(
    upload: PeerSession,
    mut prepared: PreparedFile,
    counts: Arc<Mutex<Vec<usize>>>,
    first_request: Option<Arc<tokio::sync::Barrier>>,
  ) {
    let count = prepared.seed().info.leaf_count();
    let mut bits = vec![0xff; count.div_ceil(8)];
    if !count.is_multiple_of(8) {
      *bits.last_mut().unwrap() &= 0xff << (8 - count % 8);
    }
    send(
      &*upload.control,
      Control::Bitfield {
        bits: B64.encode(bits),
      },
    )
    .await
    .unwrap();
    receive_bitfield(&*upload.control, count).await.unwrap();
    let mut first = first_request;
    loop {
      match receive(&*upload.control).await.unwrap() {
        Control::Request {
          block_index,
          count: 1,
        } => {
          if let Some(barrier) = first.take() {
            barrier.wait().await;
          }
          let index = block_index as usize;
          counts.lock().unwrap()[index] += 1;
          let bytes = prepared.read_verified_block(index).unwrap();
          send(
            &*upload.control,
            Control::Proof {
              block_index,
              count: 1,
              proof: prepared.proof(index).unwrap(),
            },
          )
          .await
          .unwrap();
          send_pieces(
            &*upload.data,
            block_index,
            &bytes,
            &[(0, bytes.len() as u32)],
          )
          .await
          .unwrap();
        }
        Control::PieceNack {
          block_index,
          piece_offset,
          length,
        } => {
          let bytes = prepared.read_verified_block(block_index as usize).unwrap();
          let ranges: Vec<_> = piece_offset.into_iter().zip(length).collect();
          send_pieces(&*upload.data, block_index, &bytes, &ranges)
            .await
            .unwrap();
        }
        Control::RequestProof {
          block_index,
          count: 1,
        } => {
          send(
            &*upload.control,
            Control::Proof {
              block_index,
              count: 1,
              proof: prepared.proof(block_index as usize).unwrap(),
            },
          )
          .await
          .unwrap();
        }
        Control::Have { .. } | Control::BlockDone { .. } => {}
        Control::Cancel { block_index, count } => {
          send(&*upload.control, Control::Cancel { block_index, count })
            .await
            .unwrap();
          assert!(matches!(
            receive(&*upload.control).await,
            Err(TransferError::Closed)
          ));
          return;
        }
        message => panic!("unexpected source message: {message:?}"),
      }
    }
  }

  #[tokio::test]
  async fn parallel_peers_request_disjoint_blocks_and_publish_durable_progress() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = vec![37; 3 * 262144 + 19];
    let prepared = source(dir.path(), &bytes);
    let seed = prepared.seed().clone();
    let path = dir.path().join("download.bin");
    let store = create_destination(&seed, &path).await.unwrap();
    let progress = crate::progress::channel(&seed, &[]);
    let observed = progress.subscribe();
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let counts = Arc::new(Mutex::new(vec![0; seed.info.leaf_count()]));
    let mut targets = Vec::new();
    let mut servers = tokio::task::JoinSet::new();
    for _ in 0..2 {
      let (download, upload) = crate::peer::test_pair(seed.info_hash()).await;
      targets.push(download);
      let prepared =
        PreparedFile::open(dir.path().join("source.bin"), "http://localhost".into()).unwrap();
      servers.spawn(observed_source(
        upload,
        prepared,
        counts.clone(),
        Some(barrier.clone()),
      ));
    }
    let output = path.clone();
    let transfer = Transfer::start(progress.clone(), async move {
      crate::scheduler::download(targets, seed, store, 2, progress, |peer| async { Ok(peer) })
        .await?;
      sync_destination(&output).await?;
      Ok(output)
    });
    let mut updates = transfer.subscribe_progress();
    let monitor = tokio::spawn(async move {
      let mut previous = 0;
      let mut maximum_active = 0;
      loop {
        let current = *updates.borrow_and_update();
        assert!(current.verified_bytes >= previous);
        assert!(current.verified_bytes <= current.total_bytes);
        previous = current.verified_bytes;
        maximum_active = maximum_active.max(current.active_peers);
        if current.state == TransferState::Completed {
          return maximum_active;
        }
        updates.changed().await.unwrap();
      }
    });
    assert_eq!(
      tokio::time::timeout(Duration::from_secs(15), transfer.wait())
        .await
        .unwrap()
        .unwrap(),
      path
    );
    assert_eq!(monitor.await.unwrap(), 2);
    while let Some(result) = servers.join_next().await {
      result.unwrap();
    }
    assert_eq!(*counts.lock().unwrap(), vec![1; 4]);
    assert_eq!(observed.borrow().state, TransferState::Completed);
    assert_eq!(observed.borrow().verified_bytes, bytes.len() as u64);
    assert_eq!(observed.borrow().active_peers, 0);
    assert_eq!(std::fs::read(path).unwrap(), bytes);
  }

  #[tokio::test]
  async fn failed_peer_requeues_earlier_block_and_late_pieces_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = vec![64; 262144 + 23];
    let prepared = source(dir.path(), &bytes);
    let seed = prepared.seed().clone();
    let path = dir.path().join("download.bin");
    let store = create_destination(&seed, &path).await.unwrap();
    let progress = crate::progress::channel(&seed, &[]);
    let (bad_download, bad) = crate::peer::test_pair(seed.info_hash()).await;
    let (good_download, good) = crate::peer::test_pair(seed.info_hash()).await;
    let (claimed, bad_claimed) = tokio::sync::oneshot::channel();
    let (completed, good_completed) = tokio::sync::oneshot::channel();
    let bad_proof = prepared.proof(0).unwrap();
    let bad_bytes = vec![255; 262144];
    let bad_server = async move {
      send(
        &*bad.control,
        Control::Bitfield {
          bits: B64.encode([0xc0]),
        },
      )
      .await
      .unwrap();
      receive_bitfield(&*bad.control, 2).await.unwrap();
      assert!(matches!(
        receive(&*bad.control).await.unwrap(),
        Control::Request { block_index: 0, .. }
      ));
      claimed.send(()).unwrap();
      good_completed.await.unwrap();
      send(
        &*bad.control,
        Control::Proof {
          block_index: 0,
          count: 1,
          proof: bad_proof,
        },
      )
      .await
      .unwrap();
      send_pieces(&*bad.data, 0, &bad_bytes, &[(0, bad_bytes.len() as u32)])
        .await
        .unwrap();
      assert!(matches!(
        receive(&*bad.control).await,
        Err(TransferError::Closed)
      ));
    };
    let good_bytes = bytes.clone();
    let good_server = async move {
      send(
        &*good.control,
        Control::Bitfield {
          bits: B64.encode([0xc0]),
        },
      )
      .await
      .unwrap();
      receive_bitfield(&*good.control, 2).await.unwrap();
      for index in [1, 0] {
        assert!(
          matches!(receive(&*good.control).await.unwrap(), Control::Request { block_index, .. } if block_index == index)
        );
        if index == 0 {
          // Late duplicates from block 1 arrive after the requeued block 0 request.
          send(
            &*good.control,
            Control::Proof {
              block_index: 1,
              count: 1,
              proof: prepared.proof(1).unwrap(),
            },
          )
          .await
          .unwrap();
          send_pieces(&*good.data, 1, &good_bytes[262144..], &[(0, 23)])
            .await
            .unwrap();
        }
        let block = if index == 0 {
          &good_bytes[..262144]
        } else {
          &good_bytes[262144..]
        };
        send(
          &*good.control,
          Control::Proof {
            block_index: index,
            count: 1,
            proof: prepared.proof(index as usize).unwrap(),
          },
        )
        .await
        .unwrap();
        send_pieces(&*good.data, index, block, &[(0, block.len() as u32)])
          .await
          .unwrap();
        assert!(matches!(
          receive(&*good.control).await.unwrap(),
          Control::Have { .. }
        ));
        assert!(matches!(
          receive(&*good.control).await.unwrap(),
          Control::BlockDone { .. }
        ));
      }
      let end = receive(&*good.control).await.unwrap();
      send(&*good.control, end).await.unwrap();
      assert!(matches!(
        receive(&*good.control).await,
        Err(TransferError::Closed)
      ));
    };
    // Signal completion of block 1 through progress, after its durable commit.
    let mut updates = progress.subscribe();
    let notify_good = async move {
      loop {
        if updates.borrow_and_update().verified_blocks == 1 {
          completed.send(()).unwrap();
          break;
        }
        updates.changed().await.unwrap();
      }
    };
    let received = crate::scheduler::download(
      vec![(bad_download, None), (good_download, Some(bad_claimed))],
      seed,
      store.clone(),
      2,
      progress.clone(),
      |(peer, gate)| async move {
        if let Some(gate) = gate {
          gate.await.unwrap();
        }
        Ok(peer)
      },
    );
    let (result, (), (), ()) = tokio::time::timeout(Duration::from_secs(15), async {
      tokio::join!(received, bad_server, good_server, notify_good)
    })
    .await
    .unwrap();
    result.unwrap();
    assert!(destination_complete(&store).unwrap());
    assert_eq!(progress.borrow().verified_blocks, 2);
    assert_eq!(progress.borrow().verified_bytes, bytes.len() as u64);
    assert_eq!(std::fs::read(path).unwrap(), bytes);
  }

  #[tokio::test]
  async fn restored_transfer_only_requests_corrupt_blocks_and_preserves_progress() {
    use std::io::{Seek, SeekFrom, Write};
    let dir = tempfile::tempdir().unwrap();
    let bytes = vec![61; 262144 + 23];
    let prepared = source(dir.path(), &bytes);
    let seed = prepared.seed().clone();
    let path = dir.path().join("download.bin");
    let mut store = crate::download_store::DownloadStore::create(&seed, &path).unwrap();
    for index in 0..2 {
      let start = index * 262144;
      store
        .commit(
          index,
          &bytes[start..(start + 262144).min(bytes.len())],
          prepared.proof(index).unwrap(),
          &seed.info.root,
        )
        .unwrap();
    }
    drop(store);
    let mut disk = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    disk.seek(SeekFrom::Start(12)).unwrap();
    disk.write_all(&[255]).unwrap();
    drop(disk);
    let restored = crate::transfer::open_destination(&seed, &path, true)
      .await
      .unwrap();
    let bits = restored.lock().unwrap().verified_bitfield();
    let progress = crate::progress::channel(&seed, &bits);
    assert_eq!(progress.borrow().verified_bytes, 23);
    assert_eq!(progress.borrow().verified_blocks, 1);
    let (download, upload) = crate::peer::test_pair(seed.info_hash()).await;
    let counts = Arc::new(Mutex::new(vec![0; 2]));
    let observed_counts = counts.clone();
    let received = crate::scheduler::download(
      vec![download],
      seed,
      restored,
      2,
      progress.clone(),
      |peer| async { Ok(peer) },
    );
    let (result, ()) = tokio::time::timeout(Duration::from_secs(10), async {
      tokio::join!(
        received,
        observed_source(upload, prepared, observed_counts, None)
      )
    })
    .await
    .unwrap();
    result.unwrap();
    assert_eq!(*counts.lock().unwrap(), vec![1, 0]);
    assert_eq!(progress.borrow().verified_bytes, bytes.len() as u64);
    assert_eq!(std::fs::read(path).unwrap(), bytes);
  }

  #[tokio::test]
  async fn cancellation_aborts_all_workers_and_respects_connection_limit() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Connecting(Arc<AtomicUsize>);
    impl Drop for Connecting {
      fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
      }
    }
    let dir = tempfile::tempdir().unwrap();
    let prepared = source(dir.path(), b"payload");
    let seed = prepared.seed().clone();
    let path = dir.path().join("download.bin");
    let store = create_destination(&seed, &path).await.unwrap();
    let retained_path = path.clone();
    let progress = crate::progress::channel(&seed, &[]);
    let observed = progress.subscribe();
    let live = Arc::new(AtomicUsize::new(0));
    let count = live.clone();
    let transfer = Transfer::start(progress.clone(), async move {
      crate::scheduler::download((0..10).collect(), seed, store, 2, progress, move |_| {
        let count = count.clone();
        async move {
          let _connecting = Connecting(count.clone());
          count.fetch_add(1, Ordering::SeqCst);
          std::future::pending::<Result<PeerSession, TransferError>>().await
        }
      })
      .await?;
      Ok(path)
    });
    tokio::time::timeout(Duration::from_secs(1), async {
      while live.load(Ordering::SeqCst) < 2 {
        tokio::task::yield_now().await;
      }
    })
    .await
    .unwrap();
    assert_eq!(live.load(Ordering::SeqCst), 2);
    transfer.cancel();
    assert_eq!(observed.borrow().state, TransferState::Cancelled);
    tokio::time::timeout(Duration::from_secs(1), async {
      while live.load(Ordering::SeqCst) != 0 {
        tokio::task::yield_now().await;
      }
    })
    .await
    .unwrap();
    assert_eq!(observed.borrow().verified_bytes, 0);
    tokio::time::timeout(Duration::from_secs(1), async {
      loop {
        if crate::download_store::DownloadStore::resume(prepared.seed(), &retained_path).is_ok() {
          break;
        }
        tokio::task::yield_now().await;
      }
    })
    .await
    .unwrap();
  }

  #[tokio::test]
  async fn successful_download_cancels_a_stalled_connection_attempt() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
      fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
      }
    }
    let dir = tempfile::tempdir().unwrap();
    let prepared = source(dir.path(), b"payload");
    let seed = prepared.seed().clone();
    let path = dir.path().join("download.bin");
    let store = create_destination(&seed, &path).await.unwrap();
    let progress = crate::progress::channel(&seed, &[]);
    let (download, upload) = crate::peer::test_pair(seed.info_hash()).await;
    let dropped = Arc::new(AtomicBool::new(false));
    let flag = dropped.clone();
    let ready = watch::channel(false).0;
    let received = crate::scheduler::download(
      vec![Some(download), None],
      seed,
      store,
      2,
      progress,
      move |peer| {
        let ready = ready.clone();
        let flag = flag.clone();
        async move {
          if let Some(peer) = peer {
            let mut started = ready.subscribe();
            while !*started.borrow_and_update() {
              started.changed().await.unwrap();
            }
            Ok(peer)
          } else {
            let _dropped = Dropped(flag);
            ready.send_modify(|value| *value = true);
            std::future::pending::<Result<PeerSession, TransferError>>().await
          }
        }
      },
    );
    let (result, served) = tokio::time::timeout(Duration::from_secs(10), async {
      tokio::join!(received, upload.serve_file(Arc::new(Mutex::new(prepared))))
    })
    .await
    .unwrap();
    result.unwrap();
    served.unwrap();
    assert!(dropped.load(Ordering::SeqCst));
    assert_eq!(std::fs::read(path).unwrap(), b"payload");
  }

  #[tokio::test]
  async fn cancellation_closes_a_peer_holding_an_unfinished_block() {
    let dir = tempfile::tempdir().unwrap();
    let prepared = source(dir.path(), b"payload");
    let seed = prepared.seed().clone();
    let path = dir.path().join("download.bin");
    let store = create_destination(&seed, &path).await.unwrap();
    let progress = crate::progress::channel(&seed, &[]);
    let observed = progress.subscribe();
    let (download, upload) = crate::peer::test_pair(seed.info_hash()).await;
    let (ready, started) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
      send(
        &*upload.control,
        Control::Bitfield {
          bits: B64.encode([0x80]),
        },
      )
      .await
      .unwrap();
      receive_bitfield(&*upload.control, 1).await.unwrap();
      assert!(matches!(
        receive(&*upload.control).await.unwrap(),
        Control::Request { .. }
      ));
      ready.send(()).unwrap();
      assert!(matches!(
        receive(&*upload.control).await,
        Err(TransferError::Closed)
      ));
    });
    let destination = path.clone();
    let transfer = Transfer::start(progress.clone(), async move {
      crate::scheduler::download(vec![download], seed, store, 1, progress, |peer| async {
        Ok(peer)
      })
      .await?;
      Ok(destination)
    });
    tokio::time::timeout(Duration::from_secs(5), started)
      .await
      .unwrap()
      .unwrap();
    assert_eq!(transfer.progress().active_peers, 1);
    transfer.cancel();
    tokio::time::timeout(Duration::from_secs(5), server)
      .await
      .unwrap()
      .unwrap();
    assert_eq!(observed.borrow().state, TransferState::Cancelled);
    assert_eq!(observed.borrow().active_peers, 0);
    assert_eq!(observed.borrow().verified_bytes, 0);
    let restored = crate::download_store::DownloadStore::resume(prepared.seed(), &path).unwrap();
    assert_eq!(restored.verified_bitfield(), vec![0]);
  }

  #[tokio::test]
  async fn all_peers_failing_publishes_failure_and_keeps_resumable_destination() {
    let dir = tempfile::tempdir().unwrap();
    let prepared = source(dir.path(), b"payload");
    let seed = prepared.seed().clone();
    let path = dir.path().join("download.bin");
    let store = create_destination(&seed, &path).await.unwrap();
    let progress = crate::progress::channel(&seed, &[]);
    let observed = progress.subscribe();
    let destination = path.clone();
    let transfer = Transfer::start(progress.clone(), async move {
      crate::scheduler::download(vec![0, 1, 2], seed, store, 2, progress, |_| async {
        Err(TransferError::Closed)
      })
      .await?;
      Ok(destination)
    });
    assert!(matches!(transfer.wait().await, Err(TransferError::Closed)));
    assert_eq!(observed.borrow().state, TransferState::Failed);
    assert_eq!(observed.borrow().verified_bytes, 0);
    assert!(crate::download_store::DownloadStore::resume(prepared.seed(), &path).is_ok());
  }

  #[tokio::test]
  async fn changed_source_withdraws_upload_and_empty_content_completes() {
    for changed in [false, true] {
      let directory = tempfile::tempdir().unwrap();
      let prepared = source(directory.path(), if changed { b"original" } else { b"" });
      let seed = prepared.seed().clone();
      if changed {
        std::fs::write(prepared.path(), b"modified").unwrap();
      }
      let (download, upload) = crate::peer::test_pair(seed.info_hash()).await;
      let (received, served) = tokio::join!(
        download.download_file(seed, directory.path().join("download.bin")),
        upload.serve_file(Arc::new(Mutex::new(prepared)))
      );
      if changed {
        assert!(received.is_err());
        assert!(matches!(
          served,
          Err(TransferError::Prepare(PrepareError::SourceChanged))
        ));
      } else {
        received.unwrap();
        served.unwrap();
      }
    }
  }
}
