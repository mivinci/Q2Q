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
  Cdn(String),
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
      Self::Cdn(e) => write!(f, "CDN fallback: {e}"),
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
      let (bytes, proof) = queue
        .guard_peer(index, self.receive_block(index, length, &completed))
        .await?;
      let root = seed.info.root;
      // Reject untrusted data before it can overwrite the destination.
      if !merkle::verify(&bytes, index, count, &proof, &root) {
        return Err(TransferError::Store(StoreError::RootMismatch));
      }
      let destination = store.clone();
      let task = lease.disk_task();
      tokio::task::spawn_blocking(move || {
        let _task = task;
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
#[path = "../tests/transfer.rs"]
mod tests;
