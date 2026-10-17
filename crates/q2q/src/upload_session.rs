//! One upload connection: channel events, file protocol, pacing and accounting.
use crate::destination::check_seed;
use crate::error::TransferError;
use crate::peer::PeerSession;
use crate::peer_transfer::{BLOCK_DEADLINE, IDLE, decode_bitfield, protocol, receive, send};
use crate::source_file::PreparedFile;
use crate::task_scope::{TaskGuard, TaskScope};
use crate::upload_progress::Connection;
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use bytes::BytesMut;
use q2q_core::block::PIECE_SIZE;
use q2q_core::merkle::Proof;
use q2q_core::peer_wire::{Control, Piece};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::OwnedSemaphorePermit;
use webrtc::data_channel::{DataChannel, DataChannelEvent};

pub(crate) struct UploadContext {
  scope: TaskScope,
  permit: Option<Arc<OwnedSemaphorePermit>>,
  rate: Option<Arc<crate::upload_rate::UploadRate>>,
  progress: Option<Arc<Connection>>,
}
impl UploadContext {
  pub(crate) fn managed(
    scope: TaskScope,
    permit: Arc<OwnedSemaphorePermit>,
    rate: Option<Arc<crate::upload_rate::UploadRate>>,
    progress: Option<Arc<Connection>>,
  ) -> Self {
    Self {
      scope,
      permit: Some(permit),
      rate,
      progress,
    }
  }
  pub(crate) fn unmanaged() -> Self {
    Self {
      scope: TaskScope::new(),
      permit: None,
      rate: None,
      progress: None,
    }
  }
  pub(crate) fn disk_task(&self) -> UploadDiskTask {
    UploadDiskTask {
      _task: self.scope.enter(),
      _permit: self.permit.clone(),
    }
  }
}
pub(crate) struct UploadDiskTask {
  // Release the slot before the final task guard wakes shutdown waiters.
  _permit: Option<Arc<OwnedSemaphorePermit>>,
  _task: TaskGuard,
}

impl PeerSession {
  /// Serve a prepared source until the downloader confirms the end of this session.
  pub async fn serve_file(self, source: Arc<Mutex<PreparedFile>>) -> Result<(), TransferError> {
    self
      .serve_file_scoped(source, UploadContext::unmanaged())
      .await
  }

  pub(crate) async fn serve_file_scoped(
    self,
    source: Arc<Mutex<PreparedFile>>,
    context: UploadContext,
  ) -> Result<(), TransferError> {
    // Poll both channel event queues even while pacing sends. In particular,
    // buffered-amount notifications must be consumed to keep the RTC driver
    // running; messages from the receiver remain bounded separately.
    let (messages, mut incoming) = tokio::sync::mpsc::channel(32);
    let control_events = async {
      loop {
        let message = receive(&*self.control).await;
        let closed = message.is_err();
        messages
          .try_send(message)
          .map_err(|_| protocol("upload control inbox overflow"))?;
        if closed {
          return std::future::pending::<Result<(), TransferError>>().await;
        }
      }
    };
    let data_events = async {
      loop {
        match self.data.poll().await {
          Some(DataChannelEvent::OnMessage(_)) => {
            return Err(protocol("unexpected upload data message"));
          }
          Some(DataChannelEvent::OnClose) | None => {
            return std::future::pending::<Result<(), TransferError>>().await;
          }
          _ => {}
        }
      }
    };
    tokio::select! {
      result = self.serve_source(source, context, &mut incoming) => result,
      result = control_events => result,
      result = data_events => result,
    }
  }

  async fn serve_source(
    &self,
    source: Arc<Mutex<PreparedFile>>,
    context: UploadContext,
    incoming: &mut tokio::sync::mpsc::Receiver<Result<Control, TransferError>>,
  ) -> Result<(), TransferError> {
    let snapshot = source.clone();
    let task = context.disk_task();
    let seed = tokio::task::spawn_blocking(move || {
      let _task = task;
      let source = snapshot
        .lock()
        .map_err(|_| protocol("source lock poisoned"))?;
      Ok::<_, TransferError>(source.seed().clone())
    })
    .await??;
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
    let message = tokio::time::timeout(IDLE, receive_upload(incoming))
      .await
      .map_err(|_| TransferError::Timeout)??;
    decode_bitfield(message, seed.info.leaf_count())?;
    let mut pending: Option<(usize, Vec<u8>, Proof)> = None;
    loop {
      let control = tokio::time::timeout(IDLE, receive_upload(incoming))
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
          if let Some(progress) = &context.progress {
            progress.state(crate::upload_progress::UploadConnectionState::Sending);
          }
          let source = source.clone();
          let task = context.disk_task();
          let (bytes, proof) = tokio::task::spawn_blocking(move || {
            let _task = task;
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
          send_pieces_limited(
            &*self.data,
            Some((&*self.control, &context)),
            block_index,
            &bytes,
            &[(0, bytes.len() as u32)],
            false,
          )
          .await?;
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
          send_pieces_limited(
            &*self.data,
            Some((&*self.control, &context)),
            block_index,
            bytes,
            &ranges,
            true,
          )
          .await?;
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
          if let Some(progress) = &context.progress {
            progress.state(crate::upload_progress::UploadConnectionState::Idle);
          }
        }
        Control::Have { .. } => {}
        Control::Cancel { block_index, count } => {
          // Acknowledge the ordered end marker before either side closes RTC.
          // It also ends sessions that supplied only a subset after failover.
          send(&*self.control, Control::Cancel { block_index, count }).await?;
          return match tokio::time::timeout(IDLE, receive_upload(incoming)).await {
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
}
async fn receive_upload(
  incoming: &mut tokio::sync::mpsc::Receiver<Result<Control, TransferError>>,
) -> Result<Control, TransferError> {
  incoming.recv().await.ok_or(TransferError::Closed)?
}

pub(crate) async fn send_pieces_limited(
  channel: &dyn DataChannel,
  pacing: Option<(&dyn DataChannel, &UploadContext)>,
  index: u64,
  bytes: &[u8],
  ranges: &[(u32, u32)],
  retransmit: bool,
) -> Result<(), TransferError> {
  let mut hints = tokio::time::interval(Duration::from_secs(1));
  hints.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
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
      if let Some((control, context)) = pacing
        && let Some(rate) = &context.rate
      {
        let budget = rate.acquire(encoded.len());
        tokio::pin!(budget);
        loop {
          tokio::select! {
            biased;
            _ = &mut budget => break,
            _ = hints.tick() => send(control, Control::UploadWait { block_index: index, retry_after: 2000 }).await?,
          }
        }
      }
      tokio::time::timeout(
        BLOCK_DEADLINE,
        channel.send(BytesMut::from(encoded.as_slice())),
      )
      .await
      .map_err(|_| TransferError::Timeout)?
      .map_err(|e| protocol(e.to_string()))?;
      if let Some((_, context)) = pacing
        && let Some(progress) = &context.progress
      {
        progress.sent(encoded.len(), retransmit);
      }
      if piece % 16 == 0 {
        tokio::task::yield_now().await;
      }
    }
  }
  Ok(())
}

#[cfg(test)]
#[path = "../tests/upload_session.rs"]
mod tests;
