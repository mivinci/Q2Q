//! Managed publication lifecycle.

pub use crate::source_file::{PrepareError, PreparedFile};
pub use crate::upload_progress::{
  PublicationState, UploadConnectionProgress, UploadConnectionState, UploadProgress,
};

use crate::control::ControlSession;
use crate::error::TransferError;
use q2q_core::merkle::Hash;
use q2q_core::seed::Seed;
use std::sync::{
  Arc,
  atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::sync::{Semaphore, oneshot, watch};
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
  progress: Arc<crate::upload_progress::Progress>,
  stop: watch::Sender<bool>,
  task: Option<JoinHandle<Result<(), TransferError>>>,
}

impl Publication {
  pub(crate) async fn start(
    prepared: PreparedFile,
    session: Arc<ControlSession>,
    active: ActivePublication,
    permits: Arc<Semaphore>,
    rate: Option<Arc<crate::upload_rate::UploadRate>>,
  ) -> Result<Self, TransferError> {
    // Claim before touching the shared Punch gate. A conflicting low-level
    // accept returns an error instead of being paused or stealing requests.
    let admission = session.claim_publication()?;
    let seed = prepared.seed().clone();
    let progress = crate::upload_progress::Progress::new();
    let (stop, stopped) = watch::channel(false);
    let (ready, started) = oneshot::channel();
    let runner = PublicationTask {
      hash: seed.info_hash(),
      source: Arc::new(prepared),
      uploads: crate::upload::UploadPool::new(permits, rate, progress.clone()),
      session,
      status: progress.run_guard(),
      progress: progress.clone(),
      admission,
      active,
    };
    let publication = Self {
      seed,
      progress,
      stop,
      task: Some(tokio::spawn(runner.run(stopped, ready))),
    };
    match started.await {
      Ok(()) => Ok(publication),
      Err(_) => {
        // The runner owns startup errors and the common cleanup path. Joining
        // it preserves the original error, including an announce failure.
        publication.close().await?;
        Err(TransferError::Protocol(
          "publication stopped before announcing".into(),
        ))
      }
    }
  }

  /// Current counters, including piece sends not yet emitted to subscribers.
  pub fn progress(&self) -> UploadProgress {
    self.progress.snapshot()
  }

  /// Coalesced snapshots (at most every 100 ms during normal operation).
  /// Closing and terminal states are sent immediately. Clone watch values
  /// before awaiting; do not hold a watch borrow across an await.
  pub fn subscribe_progress(&self) -> watch::Receiver<UploadProgress> {
    self.progress.subscribe()
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
    self.progress.closing();
    let _ = self.stop.send(true);
    let result = self.task.as_mut().expect("publication task exists").await?;
    self.task.take();
    result
  }
}

impl Drop for Publication {
  fn drop(&mut self) {
    self.progress.closing();
    let _ = self.stop.send(true);
  }
}

/// Owns every resource until withdrawal and blocking reads have finished.
/// The leases are declared in release order for cancellation/unwinding too.
struct PublicationTask {
  hash: Hash,
  source: Arc<PreparedFile>,
  uploads: crate::upload::UploadPool,
  session: Arc<ControlSession>,
  status: crate::upload_progress::RunGuard,
  progress: Arc<crate::upload_progress::Progress>,
  admission: crate::admission::AdmissionLease,
  active: ActivePublication,
}
impl PublicationTask {
  async fn run(
    mut self,
    stopped: watch::Receiver<bool>,
    ready: oneshot::Sender<()>,
  ) -> Result<(), TransferError> {
    let result = self.serve(stopped, ready).await;
    self.progress.closing();
    // All startup, stop and error paths use this order. In particular, keep
    // admission ownership until all workers/reads and withdrawal are done.
    self.session.set_upload_admission(false).await;
    self.uploads.stop().await;
    let withdrawn = self
      .session
      .update_files(&[], &[self.hash])
      .await
      .map(|_| ())
      .map_err(TransferError::Control);
    let result = result.and(withdrawn);
    drop(self.admission);
    drop(self.active);
    // A terminal snapshot permits immediate republishing: both leases have
    // already been released and no worker can still mutate the old source.
    self.progress.finish(result.is_err());
    drop(self.status);
    result
  }

  async fn serve(
    &mut self,
    mut stopped: watch::Receiver<bool>,
    ready: oneshot::Sender<()>,
  ) -> Result<(), TransferError> {
    self.session.set_upload_admission(false).await;
    self.session.set_upload_admission(true).await;
    let expires = self.session.update_files(&[self.hash], &[]).await?;
    if ready.send(()).is_err() {
      return Ok(());
    }
    tokio::select! {
      result = self.uploads.run(&self.session, self.hash, self.source.clone()) => result,
      result = renew_holding(&self.session, self.hash, expires) => result,
      error = self.session.keep_online() => Err(TransferError::Control(error)),
      _ = stopped.changed() => Ok(()),
      _ = self.progress.coalesce() => unreachable!("progress timer does not terminate"),
    }
  }
}

async fn renew_holding(
  session: &Arc<ControlSession>,
  hash: Hash,
  mut expires: u64,
) -> Result<(), TransferError> {
  loop {
    let delay = expires.saturating_sub(q2q_track::state::now()).max(2) / 2;
    tokio::time::sleep(Duration::from_secs(delay)).await;
    expires = session.update_files(&[hash], &[]).await?;
  }
}
