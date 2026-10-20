//! Peer file protocol, piece assembly and completion acknowledgements.
use crate::block_queue::WorkQueue;
use crate::destination::{Destination, check_seed};
use crate::error::TransferError;
use crate::peer::PeerSession;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use q2q_primitives::block::PIECE_SIZE;
use q2q_primitives::merkle::Proof;
use q2q_primitives::peer_wire::{Control, Piece};
use q2q_primitives::seed::Seed;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use webrtc::data_channel::{DataChannel, DataChannelEvent};
pub(crate) const IDLE: Duration = Duration::from_secs(30);
pub(crate) const BLOCK_DEADLINE: Duration = Duration::from_secs(20);
const RETRY_DELAY: Duration = Duration::from_millis(500);
impl PeerSession {
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
    let store = Destination::open(&seed, &path, false).await?;
    self.download_into(&seed, store.clone()).await?;
    store.finish().await?;
    Ok(path)
  }

  /// Continue a transfer using blocks verified in memory or revalidated from
  /// root-bound checkpoint proofs. Persisted state flags are never trusted.
  pub(crate) async fn download_into(
    self,
    seed: &Seed,
    store: Destination,
  ) -> Result<(), TransferError> {
    let bits = store.bitfield().await?;
    let progress = crate::progress::channel(seed, &bits);
    let queue = WorkQueue::new(seed, &bits, progress);
    self.download_scheduled(seed, store, queue).await
  }

  pub(crate) async fn download_scheduled(
    self,
    seed: &Seed,
    store: Destination,
    queue: Arc<WorkQueue>,
  ) -> Result<(), TransferError> {
    crate::scheduler::download_peer(self, seed, store, queue).await
  }

  async fn receive_block(
    &self,
    index: usize,
    length: usize,
    completed: &HashSet<u64>,
    available: &std::sync::Mutex<Vec<u8>>,
    count: usize,
    observation: Option<&crate::download_progress::Connection>,
  ) -> Result<(Vec<u8>, Proof), TransferError> {
    let mut assembly = Assembly::new(length);
    let mut proof = None;
    let mut retries = 0u32;
    let mut last_progress = tokio::time::Instant::now();
    let mut pacing_until = tokio::time::Instant::now();
    let idle = tokio::time::sleep(RETRY_DELAY);
    tokio::pin!(idle);
    let deadline = tokio::time::sleep(BLOCK_DEADLINE);
    tokio::pin!(deadline);
    loop {
      tokio::select! {
        _ = &mut deadline => {
          tracing::debug!(block_index = index, received_pieces = assembly.received, missing_pieces = assembly.present.len() - assembly.received, proof_received = proof.is_some(), retries, idle_ms = last_progress.elapsed().as_millis() as u64, stage = "block_receive", "Peer transfer timed out");
          return Err(TransferError::Timeout);
        },
        control = receive(&*self.control) => match control? {
          Control::Proof { block_index, count: 1, proof: received } if block_index == index as u64 => {
            if proof.is_none() { tracing::debug!(block_index = index, "Block proof received"); }
            proof = Some(received);
          },
          Control::Proof { block_index, .. } if completed.contains(&block_index) => {},
          Control::Reject { retry_after, .. } => return Err(TransferError::Rejected(retry_after)),
          Control::UploadWait { block_index, retry_after } if block_index == index as u64 => {
            let now = tokio::time::Instant::now();
            let waiting = Duration::from_millis(u64::from(retry_after));
            tracing::debug!(block_index = index, retry_after_ms = retry_after, "Uploader deferred block retries");
            pacing_until = now + waiting;
            idle.as_mut().reset(pacing_until.max(now + RETRY_DELAY));
            deadline.as_mut().reset(now + waiting.max(BLOCK_DEADLINE));
          },
          Control::UploadWait { block_index, .. } if completed.contains(&block_index) => {},
          Control::Have { block_index } => record_have(available, count, block_index)?,
          _ => return Err(protocol("unexpected download control message")),
        },
        event = self.data.poll() => match event {
          Some(DataChannelEvent::OnMessage(message)) => {
            if message.is_string { return Err(protocol("piece must be binary")); }
            let piece = Piece::decode(&message.data)?;
            if completed.contains(&piece.block_index) { continue; }
            if piece.block_index != index as u64 { return Err(protocol("unsolicited block")); }
            let fresh = assembly.insert(piece.piece_offset, piece.data)?;
            if let Some(observation) = observation { observation.received(piece.data.len()); }
            if fresh {
              if assembly.received == 1 { tracing::debug!(block_index = index, "First block piece received"); }
              retries = 0;
              let now = tokio::time::Instant::now();
              last_progress = now;
              idle.as_mut().reset(pacing_until.max(now + RETRY_DELAY));
              deadline.as_mut().reset(now + BLOCK_DEADLINE);
            }
          },
          Some(DataChannelEvent::OnClose) | None => return Err(TransferError::Closed),
          _ => {},
        },
        _ = &mut idle => {
          retries = retries.saturating_add(1);
          tracing::debug!(block_index = index, retries, missing_pieces = assembly.present.len() - assembly.received, proof_received = proof.is_some(), "Requesting missing block data");
          // Retries pace repair traffic; only the no-progress deadline ends the
          // block. Three fixed 500 ms retries prematurely killed slow TURN paths.
          tokio::time::timeout_at(deadline.deadline(), async {
            if proof.is_none() { send(&*self.control, Control::RequestProof { block_index: index as u64, count: 1 }).await?; }
            for ranges in assembly.missing().chunks(64) {
              send(&*self.control, Control::PieceNack {
                block_index: index as u64,
                piece_offset: ranges.iter().map(|(offset, _)| *offset).collect(),
                length: ranges.iter().map(|(_, length)| *length).collect(),
              }).await?;
            }
            Ok::<(), TransferError>(())
          }).await.map_err(|_| {
            tracing::debug!(block_index = index, stage = "block_retry_send", "Peer transfer timed out");
            TransferError::Timeout
          })??;
          let backoff = RETRY_DELAY * (1 << retries.min(3));
          idle.as_mut().reset(tokio::time::Instant::now() + backoff);
        },
      }
      if assembly.complete()
        && let Some(proof) = proof.take()
      {
        tracing::debug!(
          block_index = index,
          bytes = assembly.bytes.len(),
          "Block received; awaiting verification"
        );
        return Ok((assembly.bytes, proof));
      }
    }
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

pub(crate) async fn send(channel: &dyn DataChannel, message: Control) -> Result<(), TransferError> {
  tokio::time::timeout(IDLE, channel.send_text(&message.encode()?))
    .await
    .map_err(|_| TransferError::Timeout)?
    .map_err(|e| protocol(e.to_string()))
}

pub(crate) async fn receive(channel: &dyn DataChannel) -> Result<Control, TransferError> {
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
  let message = tokio::time::timeout(IDLE, receive(channel))
    .await
    .map_err(|_| TransferError::Timeout)??;
  decode_bitfield(message, count)
}

pub(crate) fn decode_bitfield(message: Control, count: usize) -> Result<Vec<u8>, TransferError> {
  let Control::Bitfield { bits } = message else {
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

#[cfg(test)]
async fn send_pieces(
  channel: &dyn DataChannel,
  index: u64,
  bytes: &[u8],
  ranges: &[(u32, u32)],
) -> Result<(), TransferError> {
  crate::upload_session::send_pieces_limited(channel, None, index, bytes, ranges, false).await
}

pub(crate) fn protocol(detail: impl Into<String>) -> TransferError {
  TransferError::Protocol(detail.into())
}

/// Protocol state for one authenticated receiver connection. It owns no queue or disk.
pub(crate) struct PeerDownload {
  session: PeerSession,
  completed: HashSet<u64>,
  available: std::sync::Mutex<Vec<u8>>,
  count: usize,
  observation: Option<Arc<crate::download_progress::Connection>>,
}
impl PeerSession {
  #[tracing::instrument(skip_all, fields(connection_id = %self.connection_id(), peer_id = %self.peer_id().as_str()))]
  pub(crate) async fn begin_download(
    self,
    seed: &Seed,
    bits: Vec<u8>,
  ) -> Result<PeerDownload, TransferError> {
    if &seed.info_hash() != self.info_hash() {
      return Err(protocol("seed belongs to another session"));
    }
    let count = seed.info.leaf_count();
    tracing::debug!(stage = "bitfield_send", "Sending download availability");
    send(
      &*self.control,
      Control::Bitfield {
        bits: B64.encode(bits),
      },
    )
    .await?;
    tracing::debug!(
      stage = "bitfield_receive",
      "Waiting for upload availability"
    );
    let available = receive_bitfield(&*self.control, count).await.inspect_err(|error| {
      tracing::debug!(%error, stage = "bitfield_receive", "Download protocol initialization failed");
    })?;
    tracing::debug!("Upload availability received");
    Ok(PeerDownload {
      session: self,
      completed: HashSet::new(),
      available: std::sync::Mutex::new(available),
      count,
      observation: None,
    })
  }
}
fn record_have(
  available: &std::sync::Mutex<Vec<u8>>,
  count: usize,
  index: u64,
) -> Result<(), TransferError> {
  let index = usize::try_from(index).map_err(|_| protocol("Have index overflows platform"))?;
  if index >= count {
    return Err(protocol("Have index out of bounds"));
  }
  available.lock().unwrap_or_else(|e| e.into_inner())[index / 8] |= 1 << (7 - index % 8);
  Ok(())
}
impl PeerDownload {
  pub(crate) fn observe(&mut self, observation: Arc<crate::download_progress::Connection>) {
    self.observation = Some(observation);
  }
  pub(crate) fn available(&self) -> Vec<u8> {
    self
      .available
      .lock()
      .unwrap_or_else(|e| e.into_inner())
      .clone()
  }
  pub(crate) async fn receive_have(&self) -> Result<(), TransferError> {
    loop {
      match receive(&*self.session.control).await? {
        Control::Have { block_index } => {
          return record_have(&self.available, self.count, block_index);
        }
        Control::Proof { block_index, .. } | Control::UploadWait { block_index, .. }
          if self.completed.contains(&block_index) => {}
        _ => return Err(protocol("expected availability update")),
      }
    }
  }

  pub(crate) async fn fetch(
    &self,
    index: usize,
    length: usize,
  ) -> Result<(Vec<u8>, Proof), TransferError> {
    tracing::debug!(
      block_index = index,
      bytes = length,
      stage = "block_request",
      "Requesting block"
    );
    send(
      &*self.session.control,
      Control::Request {
        block_index: index as u64,
        count: 1,
      },
    )
    .await?;
    self
      .session
      .receive_block(
        index,
        length,
        &self.completed,
        &self.available,
        self.count,
        self.observation.as_deref(),
      )
      .await
  }
  pub(crate) async fn acknowledge(&mut self, index: usize) -> Result<(), TransferError> {
    self.completed.insert(index as u64);
    send(
      &*self.session.control,
      Control::Have {
        block_index: index as u64,
      },
    )
    .await?;
    send(
      &*self.session.control,
      Control::BlockDone {
        block_index: index as u64,
      },
    )
    .await
  }
  pub(crate) async fn finish(self) -> Result<(), TransferError> {
    send(
      &*self.session.control,
      Control::Cancel {
        block_index: 0,
        count: 0,
      },
    )
    .await?;
    tokio::time::timeout(IDLE, async {
      loop {
        match receive(&*self.session.control).await? {
          Control::Cancel {
            block_index: 0,
            count: 0,
          } => return Ok(()),
          Control::Have { block_index } => record_have(&self.available, self.count, block_index)?,
          // A proof retry may already have been queued before BlockDone.
          Control::Proof { block_index, .. } | Control::UploadWait { block_index, .. }
            if self.completed.contains(&block_index) => {}
          _ => return Err(protocol("expected transfer end acknowledgement")),
        }
      }
    })
    .await
    .map_err(|_| TransferError::Timeout)?
  }
}

#[cfg(test)]
#[path = "../tests/transfer.rs"]
mod tests;
