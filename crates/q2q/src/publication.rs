//! Managed publication lifecycle.

pub use crate::source_file::{PrepareError, PreparedFile};

use crate::control::ControlSession;
use crate::error::TransferError;
use q2q_core::merkle::Hash;
use q2q_core::seed::Seed;
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
        error = session.keep_online() => Err(TransferError::Control(error)),
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
