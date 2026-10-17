//! One heartbeat task per active control session, retained by its users.
use crate::control::ControlError;
use std::future::Future;
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::watch;
use tokio::task::JoinHandle;

#[derive(Default)]
pub(crate) struct Online {
  current: Mutex<Weak<OnlineTask>>,
}
impl Online {
  pub(crate) fn lease(
    &self,
    operation: impl Future<Output = ControlError> + Send + 'static,
  ) -> Arc<OnlineTask> {
    let mut current = self.current.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(task) = current.upgrade() {
      return task;
    }
    let (updates, failure) = watch::channel(None);
    let task = tokio::spawn(async move {
      updates.send_replace(Some(Arc::new(operation.await)));
    });
    let online = Arc::new(OnlineTask { failure, task });
    *current = Arc::downgrade(&online);
    online
  }
}
pub(crate) struct OnlineTask {
  failure: watch::Receiver<Option<Arc<ControlError>>>,
  task: JoinHandle<()>,
}
impl OnlineTask {
  pub(crate) async fn failed(&self) -> Arc<ControlError> {
    let mut failures = self.failure.clone();
    loop {
      if let Some(error) = failures.borrow_and_update().clone() {
        return error;
      }
      if failures.changed().await.is_err() {
        return Arc::new(ControlError::Protocol(
          "heartbeat task ended without a failure result".into(),
        ));
      }
    }
  }
}
impl Drop for OnlineTask {
  fn drop(&mut self) {
    self.task.abort();
  }
}

#[cfg(test)]
#[path = "../tests/online.rs"]
mod tests;
