//! One managed Punch reader per client, dispatching bounded content mailboxes.
use crate::{control::ControlSession, error::TransferError, peer::PendingPeer};
use q2q_punch::message::RejectReason;
use std::{
  collections::HashMap,
  sync::{Arc, Mutex},
  time::Duration,
};
use tokio::{
  sync::{Mutex as AsyncMutex, mpsc, watch},
  task::{JoinHandle, JoinSet},
};

type Routes = Arc<Mutex<HashMap<[u8; 32], mpsc::Sender<PendingPeer>>>>;
#[derive(Default)]
pub(crate) struct UploadRouter {
  routes: Routes,
  run: AsyncMutex<Option<Run>>,
}
struct Run {
  session: Arc<ControlSession>,
  stop: watch::Sender<bool>,
  task: JoinHandle<()>,
}
pub(crate) struct Registration {
  router: Arc<UploadRouter>,
  hash: Option<[u8; 32]>,
  pub(crate) incoming: mpsc::Receiver<PendingPeer>,
}
impl UploadRouter {
  pub(crate) async fn register(
    self: &Arc<Self>,
    session: Arc<ControlSession>,
    hash: [u8; 32],
  ) -> Result<Registration, TransferError> {
    let router = self.clone();
    // A cancelled startup must still either return a registration guard or
    // clean it up. The detached operation's result drops that guard itself.
    tokio::spawn(async move { router.register_inner(session, hash).await }).await?
  }
  async fn register_inner(
    self: Arc<Self>,
    session: Arc<ControlSession>,
    hash: [u8; 32],
  ) -> Result<Registration, TransferError> {
    let mut run = self.run.lock().await;
    {
      let routes = self.routes.lock().unwrap_or_else(|e| e.into_inner());
      if routes.contains_key(&hash) {
        return Err(TransferError::AlreadyPublishing);
      }
      if routes.len() >= 32 {
        return Err(TransferError::Protocol(
          "at most 32 shared contents per client".into(),
        ));
      }
    }
    if run.as_ref().is_some_and(|run| run.task.is_finished()) {
      return Err(TransferError::Protocol(
        "upload router stopped; close existing publications first".into(),
      ));
    }
    if run.is_none() {
      let admission = session.claim_uploads()?;
      session.set_upload_admission(true).await;
      let (stop, mut stopped) = watch::channel(false);
      let receiving = session.clone();
      let routes = self.routes.clone();
      let task = tokio::spawn(async move {
        let _admission = admission;
        let mut rejections = JoinSet::new();
        loop {
          tokio::select! {
            biased;
            _ = stopped.changed() => break,
            _ = rejections.join_next(), if !rejections.is_empty() => {},
            request = receiving.incoming_any() => {
              let request = match request {
                Ok(request) => request,
                Err(_) => { tokio::time::sleep(Duration::from_millis(100)).await; continue; }
              };
              let target = routes.lock().unwrap_or_else(|e| e.into_inner()).get(&request.info_hash()).cloned();
              let (request, reason) = match target {
                Some(sender) => match sender.try_send(request) {
                  Ok(()) => continue,
                  Err(error) => (error.into_inner(), RejectReason::Busy),
                },
                None => (request, RejectReason::NotFound),
              };
              if rejections.len() < 8 {
                rejections.spawn(async move { let _ = tokio::time::timeout(Duration::from_secs(2), request.reject(reason)).await; });
              }
            }
          }
        }
        rejections.abort_all();
        while rejections.join_next().await.is_some() {}
      });
      *run = Some(Run {
        session,
        stop,
        task,
      });
    }
    let (sender, incoming) = mpsc::channel(16);
    self
      .routes
      .lock()
      .unwrap_or_else(|e| e.into_inner())
      .insert(hash, sender);
    drop(run);
    Ok(Registration {
      router: self,
      hash: Some(hash),
      incoming,
    })
  }
  async fn remove(&self, hash: [u8; 32]) {
    let mut run = self.run.lock().await;
    let empty = {
      let mut routes = self.routes.lock().unwrap_or_else(|e| e.into_inner());
      routes.remove(&hash);
      routes.is_empty()
    };
    if empty && let Some(run) = run.take() {
      // Keep serialization through shutdown: a new registration cannot reopen
      // the gate until the old reader and its managed admission lease are gone.
      run.session.set_upload_admission(false).await;
      let _ = run.stop.send(true);
      let _ = run.task.await;
    }
  }
}
impl Registration {
  pub(crate) async fn close(mut self) {
    if let Some(hash) = self.hash.take() {
      self.router.remove(hash).await;
    }
  }
}
impl Drop for Registration {
  fn drop(&mut self) {
    if let Some(hash) = self.hash.take() {
      let router = self.router.clone();
      tokio::spawn(async move {
        router.remove(hash).await;
      });
    }
  }
}
