//! Public download task handle and progress observation.
pub use crate::download_progress::{DownloadPeerProgress, DownloadPeerState};
pub use crate::error::TransferError;
pub use crate::progress::{TransferProgress, TransferState};
use std::future::Future;
use std::path::PathBuf;
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// A download task. Dropping or cancelling it stops the transfer; an incomplete
/// destination remains unverified and `wait` never reports it as complete.
pub struct Transfer {
  pub(crate) task: Option<JoinHandle<Result<PathBuf, TransferError>>>,
  pub(crate) progress: watch::Sender<TransferProgress>,
  pub(crate) peers: std::sync::Arc<crate::download_progress::Progress>,
}
impl Transfer {
  pub(crate) fn start(
    progress: watch::Sender<TransferProgress>,
    operation: impl Future<Output = Result<PathBuf, TransferError>> + Send + 'static,
  ) -> Self {
    Self::observed(
      progress,
      crate::download_progress::Progress::new(),
      operation,
    )
  }

  pub(crate) fn observed(
    progress: watch::Sender<TransferProgress>,
    peers: std::sync::Arc<crate::download_progress::Progress>,
    operation: impl Future<Output = Result<PathBuf, TransferError>> + Send + 'static,
  ) -> Self {
    let updates = progress.clone();
    let peer_updates = peers.clone();
    let task = tokio::spawn(async move {
      tokio::pin!(operation);
      let result = tokio::select! {
        result = &mut operation => result,
        _ = peer_updates.coalesce() => unreachable!("coalescing loop does not end"),
      };
      peer_updates.close();
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
      peers,
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

  /// Live attempts only, bounded by download concurrency. No finished-peer history.
  pub fn peer_progress(&self) -> Vec<DownloadPeerProgress> {
    self.peers.snapshot()
  }

  /// Coalesced received-byte updates (100 ms); topology/state changes are immediate.
  /// Clone the snapshot before awaiting. Terminal snapshots are empty.
  pub fn subscribe_peer_progress(&self) -> watch::Receiver<Vec<DownloadPeerProgress>> {
    self.peers.subscribe()
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
    self.peers.close();
    if let Some(task) = &self.task {
      task.abort();
      crate::progress::finish(&self.progress, TransferState::Cancelled);
    }
  }
}
