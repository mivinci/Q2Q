//! Public download task handle and progress observation.
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
