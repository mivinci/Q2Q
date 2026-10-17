//! Bounded standing-channel dispatch: Connect admission and isolated mailboxes.
use crate::control::ControlError;
use q2q_punch::message::{SignalRequest, SignalType};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::sync::{Mutex as AsyncMutex, mpsc, watch};
use tokio::task::{JoinHandle, JoinSet};

const INCOMING: usize = 16;
const MAILBOX: usize = 32;
const ROUTES: usize = 32;
const REJECTIONS: usize = 8;
type Reject = Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;
type Routes = Arc<Mutex<HashMap<String, Weak<Mailbox>>>>;

pub(crate) struct IncomingSignal {
  pub(crate) connect: SignalRequest,
  pub(crate) route: SignalRoute,
}
pub(crate) struct SignalRoute(Arc<Mailbox>);
struct Mailbox {
  id: String,
  routes: Weak<Mutex<HashMap<String, Weak<Mailbox>>>>,
  sender: mpsc::Sender<SignalRequest>,
  messages: AsyncMutex<mpsc::Receiver<SignalRequest>>,
  failure: watch::Sender<Option<Arc<ControlError>>>,
}
impl SignalRoute {
  pub(crate) async fn recv(&self) -> Result<SignalRequest, ControlError> {
    let mut failed = self.0.failure.subscribe();
    let mut messages = self.0.messages.lock().await;
    loop {
      if let Some(error) = failed.borrow_and_update().clone() {
        return Err(ControlError::Shared(error));
      }
      tokio::select! {
        biased;
        _ = failed.changed() => {},
        message = messages.recv() => return message.ok_or_else(|| ControlError::Transport("signal mailbox closed".into())),
      }
    }
  }
}
impl Drop for Mailbox {
  fn drop(&mut self) {
    if let Some(routes) = self.routes.upgrade() {
      let mut routes = routes.lock().unwrap_or_else(|e| e.into_inner());
      if routes
        .get(&self.id)
        .is_some_and(|route| std::ptr::eq(route.as_ptr(), self))
      {
        routes.remove(&self.id);
      }
    }
  }
}
struct Shutdown(Routes);
impl Drop for Shutdown {
  fn drop(&mut self) {
    let routes: Vec<_> = self
      .0
      .lock()
      .unwrap_or_else(|e| e.into_inner())
      .values()
      .filter_map(Weak::upgrade)
      .collect();
    for route in routes {
      route.failure.send_if_modified(|failure| {
        if failure.is_some() {
          return false;
        }
        *failure = Some(Arc::new(ControlError::Transport(
          "Punch signal router stopped".into(),
        )));
        true
      });
    }
  }
}

pub(crate) struct SignalRouter {
  incoming: AsyncMutex<mpsc::Receiver<IncomingSignal>>,
  failure: watch::Receiver<Option<Arc<ControlError>>>,
  task: JoinHandle<()>,
  accepting: Arc<Mutex<bool>>,
  admission_changed: watch::Sender<u64>,
  reject: Reject,
}
impl SignalRouter {
  pub(crate) fn new<R, RFut, W, WFut>(read: R, reject: W) -> Self
  where
    R: Fn() -> RFut + Send + 'static,
    RFut: Future<Output = Result<SignalRequest, ControlError>> + Send,
    W: Fn(String) -> WFut + Send + Sync + 'static,
    WFut: Future<Output = ()> + Send + 'static,
  {
    let reject: Reject = Arc::new(move |id| Box::pin(reject(id)));
    let rejecting = reject.clone();
    let accepting = Arc::new(Mutex::new(true));
    let admission = accepting.clone();
    let (sender, incoming) = mpsc::channel(INCOMING);
    let (failed, failure) = watch::channel(None);
    let routes: Routes = Arc::new(Mutex::new(HashMap::new()));
    let task = tokio::spawn(async move {
      let _shutdown = Shutdown(routes.clone());
      let mut rejections = JoinSet::new();
      loop {
        let message = tokio::select! {
          message = read() => match message {
            Ok(message) => message,
            Err(error) => { let error = Arc::new(error);
              failed.send_replace(Some(error.clone()));
              let active: Vec<_> = routes.lock().unwrap_or_else(|e| e.into_inner()).values().filter_map(Weak::upgrade).collect();
              for route in active { route.failure.send_replace(Some(error.clone())); }
              break; }
          },
          _ = rejections.join_next(), if !rejections.is_empty() => continue,
        };
        if message.kind == SignalType::Connect {
          let id = message.connection_id.clone();
          let existing = routes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id)
            .and_then(Weak::upgrade);
          if existing.is_some() {
            continue;
          }
          let accepting = admission.lock().unwrap_or_else(|e| e.into_inner());
          let full =
            !*accepting || routes.lock().unwrap_or_else(|e| e.into_inner()).len() >= ROUTES;
          if !full {
            let (tx, rx) = mpsc::channel(MAILBOX);
            let route = Arc::new(Mailbox {
              id: id.clone(),
              routes: Arc::downgrade(&routes),
              sender: tx,
              messages: AsyncMutex::new(rx),
              failure: watch::channel(None).0,
            });
            routes
              .lock()
              .unwrap_or_else(|e| e.into_inner())
              .insert(id.clone(), Arc::downgrade(&route));
            let request = IncomingSignal {
              connect: message,
              route: SignalRoute(route),
            };
            if sender.try_send(request).is_ok() {
              continue;
            }
          }
          drop(accepting);
          if rejections.len() < REJECTIONS {
            let rejecting = rejecting(id);
            rejections.spawn(async move {
              let _ = tokio::time::timeout(Duration::from_secs(2), rejecting).await;
            });
          }
        } else {
          let route = routes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&message.connection_id)
            .and_then(Weak::upgrade);
          if let Some(route) = route
            && route.sender.try_send(message).is_err()
          {
            route
              .failure
              .send_replace(Some(Arc::new(ControlError::Protocol(
                "signal mailbox overflow".into(),
              ))));
          }
        }
      }
    });
    Self {
      incoming: AsyncMutex::new(incoming),
      failure,
      task,
      accepting,
      admission_changed: watch::channel(0).0,
      reject,
    }
  }
  // Managed publications pause admission between lifetimes. Drain buffered
  // requests too, so abandoned readers cannot reserve the next source's slots.
  pub(crate) async fn set_accepting(&self, enabled: bool) {
    {
      let mut accepting = self.accepting.lock().unwrap_or_else(|e| e.into_inner());
      if *accepting != enabled {
        *accepting = enabled;
        // Notify before acquiring the receiver lock: next() holds it while
        // waiting for a request. The generation also remembers a quick resume.
        self
          .admission_changed
          .send_modify(|generation| *generation = generation.wrapping_add(1));
      }
    }
    if enabled {
      return;
    }
    let pending = {
      let mut incoming = self.incoming.lock().await;
      let mut pending = Vec::new();
      while let Ok(request) = incoming.try_recv() {
        pending.push(request.connect.connection_id);
      }
      pending
    };
    let mut rejecting = JoinSet::new();
    for id in pending {
      let reject = (self.reject)(id);
      rejecting.spawn(async move {
        let _ = tokio::time::timeout(Duration::from_secs(2), reject).await;
      });
    }
    while rejecting.join_next().await.is_some() {}
  }
  pub(crate) async fn next(&self) -> Result<IncomingSignal, ControlError> {
    let mut failure = self.failure.clone();
    let mut admission = self.admission_changed.subscribe();
    let mut incoming = tokio::select! {
      biased;
      _ = admission.changed() => return Err(ControlError::AdmissionPaused),
      incoming = self.incoming.lock() => incoming,
    };
    if !*self.accepting.lock().unwrap_or_else(|e| e.into_inner()) {
      return Err(ControlError::AdmissionPaused);
    }
    loop {
      if let Some(error) = failure.borrow_and_update().clone() {
        return Err(ControlError::Shared(error));
      }
      tokio::select! {
        biased;
        _ = admission.changed() => return Err(ControlError::AdmissionPaused),
        result = failure.changed() => if result.is_err() { return Err(ControlError::Transport("signal router stopped".into())); },
        request = incoming.recv() => return request.ok_or_else(|| ControlError::Transport("signal router closed".into())),
      }
    }
  }
}
impl Drop for SignalRouter {
  fn drop(&mut self) {
    self.task.abort();
  }
}

#[cfg(test)]
#[path = "../tests/signal_router.rs"]
mod tests;
