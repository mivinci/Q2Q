//! Source selection rules, independent of leases, tasks and disk access.
use crate::error::TransferError;
use std::sync::Arc;

use std::time::Duration;
use tokio::time::Instant;

#[derive(Clone, Copy)]
pub(crate) enum SourceKind {
  Peer,
  Cdn,
}

#[derive(Default)]
pub(crate) struct DownloadPolicy {
  window: Option<Window>,
}
struct Window {
  start: usize,
  end: usize,
  urgent: Option<(usize, Instant)>,
  cdn_ready: bool,
  peers_exhausted: bool,
  errors: Vec<Option<Arc<TransferError>>>,
  source_error: Option<Arc<TransferError>>,
  fatal: Option<Arc<TransferError>>,
}
impl DownloadPolicy {
  pub(crate) fn window(count: usize) -> Self {
    Self {
      window: Some(Window {
        start: 0,
        end: 0,
        urgent: None,
        cdn_ready: false,
        peers_exhausted: false,
        errors: vec![None; count],
        source_error: None,
        fatal: None,
      }),
    }
  }
  pub(crate) fn focus(&mut self, index: usize, end: usize, wait: Duration, urgent: bool) {
    if let Some(window) = &mut self.window {
      window.start = index;
      window.end = end;
      window.urgent = urgent.then(|| (index, Instant::now() + wait));
      for error in window.errors.iter_mut().take(end).skip(index) {
        *error = None;
      }
    }
  }
  pub(crate) fn contains(&self, index: usize) -> bool {
    self
      .window
      .as_ref()
      .is_none_or(|w| index >= w.start && index < w.end)
  }
  pub(crate) fn peer_status(&mut self, exhausted: bool, error: Option<TransferError>) {
    if let Some(w) = &mut self.window {
      w.peers_exhausted = exhausted;
      if let Some(e) = error {
        w.source_error = Some(Arc::new(e));
      }
    }
  }
  pub(crate) fn cdn_status(&mut self, result: Result<(), TransferError>) {
    if let Some(w) = &mut self.window {
      match result {
        Ok(()) => w.cdn_ready = true,
        Err(error) => {
          w.cdn_ready = false;
          w.source_error = Some(Arc::new(error));
        }
      }
    }
  }
  pub(crate) fn fail(&mut self, error: TransferError) {
    if let Some(w) = &mut self.window {
      w.fatal = Some(Arc::new(error));
    }
  }
  pub(crate) fn fatal(&self) -> Option<&Arc<TransferError>> {
    self.window.as_ref().and_then(|w| w.fatal.as_ref())
  }
  pub(crate) fn block_failed(&mut self, index: usize, error: TransferError) {
    if let Some(w) = &mut self.window {
      w.errors[index] = Some(Arc::new(error));
    }
  }
  pub(crate) fn read_error(&self, index: usize) -> Option<&Arc<TransferError>> {
    self
      .window
      .as_ref()
      .filter(|w| w.peers_exhausted)
      .and_then(|w| {
        w.errors[index]
          .as_ref()
          .or(w.source_error.as_ref().filter(|_| !w.cdn_ready))
      })
  }
  pub(crate) fn deadline(&self, done: &[bool]) -> Option<Instant> {
    self
      .window
      .as_ref()
      .and_then(|w| w.urgent)
      .filter(|(i, _)| !done[*i])
      .map(|(_, deadline)| deadline)
  }
  pub(crate) fn peers_exhausted(&self) -> bool {
    self.window.as_ref().is_some_and(|w| w.peers_exhausted)
  }
  pub(crate) fn peer_deadline(&self, index: usize) -> Option<Instant> {
    self
      .window
      .as_ref()
      .filter(|w| w.cdn_ready && w.errors[index].is_none())
      .and_then(|w| w.urgent)
      .filter(|(i, _)| *i == index)
      .map(|(_, deadline)| deadline)
  }
  pub(crate) fn eligible(&self, index: usize, source: SourceKind) -> bool {
    match source {
      SourceKind::Peer => self
        .peer_deadline(index)
        .is_none_or(|deadline| Instant::now() < deadline),
      SourceKind::Cdn => self.window.as_ref().is_none_or(|w| {
        w.errors[index].is_none()
          && (w.peers_exhausted
            || w
              .urgent
              .is_some_and(|(i, deadline)| i == index && Instant::now() >= deadline))
      }),
    }
  }
}
