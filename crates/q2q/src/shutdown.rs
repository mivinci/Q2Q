use q2q_core::client::PublishProgress;
use std::{future::Future, io, pin::Pin};
use tokio::sync::watch;

pub(crate) type Stop = Pin<Box<dyn Future<Output = io::Result<()>> + Send>>;

pub(crate) fn listen() -> io::Result<Stop> {
  #[cfg(unix)]
  {
    use tokio::signal::unix::{SignalKind, signal};
    // Register before initialization so signals received while preparing a file
    // remain queued until its newly created handle can be explicitly closed.
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    Ok(Box::pin(async move {
      tokio::select! {
        _ = interrupt.recv() => Ok(()),
        _ = terminate.recv() => Ok(()),
      }
    }))
  }
  #[cfg(not(unix))]
  {
    Ok(Box::pin(tokio::signal::ctrl_c()))
  }
}

pub(crate) async fn initialize<T>(
  operation: impl Future<Output = crate::Result<T>>,
  stop: &mut Stop,
) -> crate::Result<(T, bool)> {
  tokio::pin!(operation);
  tokio::select! {
    result = &mut operation => Ok((result?, false)),
    signal = stop => {
      signal?;
      eprintln!("Stopping after initialization completes...");
      // Do not drop a startup future that may already own a Publication runner.
      // Finish acquiring its handle, then let the caller close/cancel it.
      Ok((operation.await?, true))
    }
  }
}

pub(crate) async fn initialize_with_progress<T>(
  operation: impl Future<Output = crate::Result<T>>,
  stop: &mut Stop,
  mut updates: watch::Receiver<PublishProgress>,
  mut render: impl FnMut(PublishProgress),
) -> crate::Result<(T, bool)> {
  tokio::pin!(operation);
  let mut interrupted = false;
  let mut updates_open = true;
  loop {
    tokio::select! {
      biased;
      signal = stop.as_mut(), if !interrupted => {
        signal?;
        interrupted = true;
        eprintln!("Stop requested; finishing initialization so Q2Q can close an acquired publication. Further signals during initialization and cleanup are ignored.");
      }
      result = &mut operation => return Ok((result?, interrupted)),
      changed = updates.changed(), if updates_open => {
        match changed {
          Ok(()) => {
            let progress = *updates.borrow_and_update();
            render(progress);
          }
          Err(_) => updates_open = false,
        }
      }
    }
  }
}

#[cfg(test)]
#[path = "../tests/shutdown.rs"]
mod tests;
