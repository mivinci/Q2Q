//! Content available for upload, independent of publication and transport.
use crate::error::TransferError;
use crate::source_file::PreparedFile;
use q2q_core::{merkle::Proof, seed::Seed};
use std::sync::Arc;
use tokio::sync::{Semaphore, watch};

#[derive(Clone)]
pub(crate) struct Availability {
  pub(crate) bits: Vec<u8>,
  pub(crate) failed: bool,
  pub(crate) complete: bool,
}
impl Availability {
  pub(crate) fn has(&self, index: usize) -> bool {
    !self.failed
      && self
        .bits
        .get(index / 8)
        .is_some_and(|byte| byte & (1 << (7 - index % 8)) != 0)
  }
  pub(crate) fn any(&self) -> bool {
    !self.failed && self.bits.iter().any(|byte| *byte != 0)
  }
}

pub(crate) trait UploadSource: Send + Sync {
  fn seed(&self) -> &Seed;
  fn availability(&self) -> watch::Receiver<Availability>;
  fn read_limit(&self) -> Arc<Semaphore>;
  /// Runs on a blocking thread with a read permit. Data and proof must refer
  /// to the same verified block; sparse or merely received bytes are excluded.
  fn read_block(&self, index: usize) -> Result<(Vec<u8>, Proof), TransferError>;
}
impl UploadSource for PreparedFile {
  fn seed(&self) -> &Seed {
    self.seed()
  }
  fn availability(&self) -> watch::Receiver<Availability> {
    let count = self.seed().info.leaf_count();
    let mut bits = vec![0; count.div_ceil(8)];
    for index in 0..count {
      bits[index / 8] |= 1 << (7 - index % 8);
    }
    watch::channel(Availability {
      bits,
      failed: false,
      complete: true,
    })
    .1
  }
  fn read_limit(&self) -> Arc<Semaphore> {
    self.read_limit()
  }
  fn read_block(&self, index: usize) -> Result<(Vec<u8>, Proof), TransferError> {
    Ok((self.read_verified_block(index)?, self.proof(index)?))
  }
}

#[cfg(test)]
#[path = "../tests/upload_source.rs"]
mod tests;
