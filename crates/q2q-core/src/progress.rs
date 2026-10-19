//! Coalesced transfer snapshots; consumers do not backpressure disk/network I/O.

use q2q_primitives::seed::Seed;
use tokio::sync::watch;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferState {
  Downloading,
  Completed,
  Failed,
  Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferProgress {
  pub total_bytes: u64,
  pub verified_bytes: u64,
  pub total_blocks: usize,
  pub verified_blocks: usize,
  pub active_peers: usize,
  /// In-flight HTTP data or leaf-table requests, distinct from WebRTC peers.
  pub active_cdn_requests: usize,
  pub state: TransferState,
}

pub(crate) fn channel(seed: &Seed, bits: &[u8]) -> watch::Sender<TransferProgress> {
  let mut progress = TransferProgress {
    total_bytes: seed.info.length,
    verified_bytes: 0,
    total_blocks: seed.info.leaf_count(),
    verified_blocks: 0,
    active_peers: 0,
    active_cdn_requests: 0,
    state: TransferState::Downloading,
  };
  for index in 0..progress.total_blocks {
    if bits
      .get(index / 8)
      .is_some_and(|byte| byte & (1 << (7 - index % 8)) != 0)
    {
      progress.verified_blocks += 1;
      progress.verified_bytes += block_len(seed.info.length, seed.info.block_size, index);
    }
  }
  watch::channel(progress).0
}

pub(crate) fn block_len(length: u64, block_size: u32, index: usize) -> u64 {
  (length - index as u64 * u64::from(block_size)).min(u64::from(block_size))
}

pub(crate) fn finish(progress: &watch::Sender<TransferProgress>, state: TransferState) {
  progress.send_modify(|p| {
    if p.state == TransferState::Downloading {
      p.state = state;
    }
  });
}

pub(crate) struct ActivePeer(watch::Sender<TransferProgress>);
impl ActivePeer {
  pub(crate) fn new(progress: watch::Sender<TransferProgress>) -> Self {
    progress.send_modify(|p| p.active_peers += 1);
    Self(progress)
  }
}
impl Drop for ActivePeer {
  fn drop(&mut self) {
    self.0.send_modify(|p| p.active_peers -= 1);
  }
}

pub(crate) struct ActiveCdnRequest(watch::Sender<TransferProgress>);
impl ActiveCdnRequest {
  pub(crate) fn new(progress: watch::Sender<TransferProgress>) -> Self {
    progress.send_modify(|p| p.active_cdn_requests += 1);
    Self(progress)
  }
}
impl Drop for ActiveCdnRequest {
  fn drop(&mut self) {
    self.0.send_modify(|p| p.active_cdn_requests -= 1);
  }
}
