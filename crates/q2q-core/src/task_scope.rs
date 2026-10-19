use tokio::sync::watch;

/// Tracks network workers and their non-abortable disk jobs across source phases.
#[derive(Clone)]
pub(crate) struct TaskScope(watch::Sender<usize>);
impl TaskScope {
  pub(crate) fn new() -> Self {
    Self(watch::channel(0).0)
  }
  pub(crate) fn enter(&self) -> TaskGuard {
    self.0.send_modify(|count| *count += 1);
    TaskGuard {
      scope: self.clone(),
    }
  }
  pub(crate) async fn idle(&self) {
    let mut count = self.0.subscribe();
    loop {
      if *count.borrow_and_update() == 0 {
        return;
      }
      count.changed().await.expect("scope retains its sender");
    }
  }
}
pub(crate) struct TaskGuard {
  scope: TaskScope,
}
impl Drop for TaskGuard {
  fn drop(&mut self) {
    self.scope.0.send_modify(|count| *count -= 1);
  }
}
