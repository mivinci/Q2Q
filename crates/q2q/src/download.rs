//! Download orchestration shared by full-file transfers and seekable readers.
use crate::block_queue::WorkQueue;
use crate::client::Client;
use crate::control::{ControlError, ControlSession};
use crate::destination::Destination;
use crate::error::TransferError;
use crate::progress::{self, TransferState};
use crate::transfer::Transfer;
use q2q_core::seed::Seed;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::{JoinHandle, JoinSet};

pub(crate) async fn start(
  client: &Client,
  seed: &Seed,
  path: PathBuf,
  resume: bool,
  concurrency: usize,
) -> Result<Transfer, TransferError> {
  crate::destination::check_seed(seed)?;
  let initial_store = if resume {
    Some(Destination::open(seed, &path, true).await?)
  } else {
    None
  };
  let bits = match &initial_store {
    Some(store) => store.bitfield().await?,
    None => Vec::new(),
  };
  let progress = crate::progress::channel(seed, &bits);
  if let Some(store) = &initial_store
    && store.complete().await?
  {
    return Ok(Transfer::start(progress, async move {
      initial_store.expect("resumed destination").finish().await?;
      Ok(path)
    }));
  }
  if !resume && tokio::fs::try_exists(&path).await? {
    return Err(
      std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "download destination already exists",
      )
      .into(),
    );
  }
  if seed.info.length == 0 {
    let seed = seed.clone();
    return Ok(Transfer::start(progress, async move {
      let store = Destination::open(&seed, &path, false).await?;
      store.finish().await?;
      Ok(path)
    }));
  }
  // A failed control plane must not prevent CDN-only cold start. Bound the
  // complete discovery path, including Punch Binding setup, before fallback.
  let discovery = discover(client.connect_owned(), client.peer_id().to_string(), seed).await;
  let discovery = match discovery {
    Err(error) if seed.cdn_list.is_empty() => return Err(error),
    result => result,
  };
  let seed = seed.clone();
  let updates = progress.clone();
  Ok(Transfer::start(progress, async move {
    let store = match initial_store {
      Some(store) => store,
      None => Destination::open(&seed, &path, false).await?,
    };
    let activity = crate::scheduler::TaskScope::new();
    let p2p = match discovery {
      Ok((session, targets)) => {
        let dialing = session.clone();
        let info_hash = seed.info_hash();
        let attempt = crate::scheduler::download_scoped(
          targets,
          seed.clone(),
          store.clone(),
          concurrency,
          updates.clone(),
          move |target| {
            let session = dialing.clone();
            async move {
              session
                .dial_peer(&target, info_hash)
                .await
                .map_err(Into::into)
            }
          },
          activity.clone(),
        );
        tokio::select! {
          result = attempt => result,
          result = session.keep_online() => Err(TransferError::Control(result)),
        }
      }
      Err(error) => Err(error),
    };
    if let Err(error) = p2p {
      if seed.cdn_list.is_empty()
        || !(error.peer_retryable() || matches!(error, TransferError::NoPeers))
      {
        return Err(error);
      }
      // Workers can be cancelled while blocking writes remain queued. Wait
      // for both before snapshotting state for a new source phase.
      activity.idle().await;
      cdn_all(seed, store.clone(), concurrency, updates).await?;
    }
    store.finish().await?;
    Ok(path)
  }))
}

pub(crate) struct ReadSession {
  pub(crate) seed: Arc<Seed>,
  pub(crate) store: Destination,
  pub(crate) queue: Arc<WorkQueue>,
  driver: JoinHandle<()>,
}
impl Drop for ReadSession {
  fn drop(&mut self) {
    self.driver.abort();
    progress::finish(&self.queue.progress, TransferState::Cancelled);
  }
}
pub(crate) async fn open(
  client: &Client,
  seed: Seed,
  path: PathBuf,
  concurrency: usize,
) -> Result<ReadSession, TransferError> {
  let resume = tokio::fs::try_exists(&path).await?;
  let store = Destination::open(&seed, &path, resume).await?;
  let bits = store.bitfield().await?;
  let progress = progress::channel(&seed, &bits);
  let queue = WorkQueue::demand(&seed, &bits, progress);
  let seed = Arc::new(seed);
  let dial = client.connect_owned();
  let peer_id = client.peer_id().to_string();
  let task_queue = queue.clone();
  let destination = store.clone();
  let task_seed = seed.clone();
  let finishing = store.clone();
  let driver = tokio::spawn(async move {
    let mut tasks = JoinSet::new();
    let q = task_queue.clone();
    let s = task_seed.clone();
    let disk = destination.clone();
    tasks.spawn(async move {
      let outcome = peers(dial, peer_id, s, disk, q.clone(), concurrency).await;
      if let Err(error) = outcome {
        if error.peer_retryable() || matches!(error, TransferError::NoPeers) {
          q.peer_status(true, None);
        } else {
          q.fail(error);
        }
      }
    });
    let q = task_queue.clone();
    tasks.spawn(async move {
      cdn_window(task_seed, destination, q, concurrency).await;
    });
    while let Some(result) = tasks.join_next().await {
      if let Err(error) = result {
        task_queue.fail(error.into());
        return;
      }
    }
    if task_queue.complete() {
      match finishing.finish().await {
        Ok(()) => progress::finish(&task_queue.progress, TransferState::Completed),
        Err(error) => task_queue.fail(error),
      }
    }
  });
  Ok(ReadSession {
    seed,
    store,
    queue,
    driver,
  })
}

async fn discover(
  connect: impl Future<Output = Result<Arc<ControlSession>, ControlError>>,
  peer_id: String,
  seed: &Seed,
) -> Result<(Arc<ControlSession>, Vec<q2q_track::wire::PeerWire>), TransferError> {
  tokio::time::timeout(Duration::from_secs(15), async {
    let session = connect.await?;
    let response = session.query(&seed.info_hash()).await?;
    let mut seen = std::collections::HashSet::new();
    let targets: Vec<_> = response
      .peers
      .into_iter()
      .filter(|p| p.complete && p.peer_id != peer_id && seen.insert(p.peer_id.clone()))
      .collect();
    if targets.is_empty() {
      return Err(TransferError::NoPeers);
    }
    Ok((session, targets))
  })
  .await
  .map_err(|_| TransferError::Timeout)?
}
async fn peers(
  connect: impl Future<
    Output = Result<Arc<crate::control::ControlSession>, crate::control::ControlError>,
  >,
  peer_id: String,
  seed: Arc<Seed>,
  store: Destination,
  queue: Arc<WorkQueue>,
  concurrency: usize,
) -> Result<(), TransferError> {
  if queue.complete() {
    return Ok(());
  }
  let (session, targets) = tokio::select! {
    _ = queue.finished() => return Ok(()),
    result = discover(connect, peer_id, &seed) => result?,
  };
  if targets.is_empty() {
    return Err(TransferError::NoPeers);
  }
  let manager = async {
    let mut focus = queue.subscribe_focus();
    loop {
      queue.peer_status(false, None);
      let dialing = session.clone();
      let info_hash = seed.info_hash();
      crate::scheduler::run_peers(
        targets.clone(),
        seed.clone(),
        store.clone(),
        queue.clone(),
        concurrency,
        move |target| {
          let session = dialing.clone();
          async move {
            session
              .dial_peer(&target, info_hash)
              .await
              .map_err(Into::into)
          }
        },
      )
      .await
      .or_else(|error| {
        if error.peer_retryable() || matches!(error, TransferError::NoPeers) {
          Ok(())
        } else {
          Err(error)
        }
      })?;
      if queue.complete() {
        return Ok(());
      }
      queue.peer_status(true, None);
      tokio::select! { _ = queue.finished() => return Ok(()), _ = focus.changed() => {} }
      focus.borrow_and_update();
    }
  };
  tokio::select! {
    result = manager => result,
    error = session.keep_online() => Err(TransferError::Control(error)),
  }
}

pub(crate) async fn cdn_window(
  seed: Arc<Seed>,
  store: Destination,
  queue: Arc<WorkQueue>,
  concurrency: usize,
) {
  if queue.complete() {
    return;
  }
  if seed.cdn_list.is_empty() {
    queue.cdn_status(Err(TransferError::Cdn(
      "no CDN sources configured and peers unavailable".into(),
    )));
    return;
  }
  tokio::select! { _ = queue.finished() => return, _ = queue.cdn_needed() => {} }
  if queue.complete() {
    return;
  }
  let source = match crate::cdn::Source::load(&seed, &queue.progress).await {
    Ok(source) => Arc::new(source),
    Err(error) => {
      queue.cdn_status(Err(error));
      return;
    }
  };
  queue.cdn_status(Ok(()));
  let mut tasks = JoinSet::new();
  for _ in 0..concurrency {
    let seed = seed.clone();
    let store = store.clone();
    let queue = queue.clone();
    let source = source.clone();
    tasks.spawn(async move {
      while let Some(lease) = queue.claim_cdn().await {
        let index = lease.index;
        let (bytes, proof) = match source.fetch(&seed, index, &queue.progress).await {
          Ok(block) => block,
          Err(error) => {
            queue.block_failed(index, error);
            drop(lease);
            continue;
          }
        };
        store.commit(lease, bytes, proof).await?;
      }
      Ok::<_, TransferError>(())
    });
  }
  while let Some(result) = tasks.join_next().await {
    match result {
      Ok(Ok(())) => {}
      Ok(Err(error)) => {
        queue.fail(error);
        return;
      }
      Err(error) => {
        queue.fail(error.into());
        return;
      }
    }
  }
}

/// Whole-file policy retires failing workers on one mirror before trying the next.
pub(crate) async fn cdn_all(
  seed: Seed,
  store: Destination,
  concurrency: usize,
  progress: tokio::sync::watch::Sender<crate::progress::TransferProgress>,
) -> Result<(), TransferError> {
  let bits = store.bitfield().await?;
  let queue = WorkQueue::new(&seed, &bits, progress.clone());
  if queue.complete() {
    return Ok(());
  }
  let source = crate::cdn::Source::load(&seed, &progress).await?;
  let seed = Arc::new(seed);
  let mut last = TransferError::Cdn("no usable CDN source".into());
  for mirror in source.mirrors() {
    let mut workers = JoinSet::new();
    for _ in 0..concurrency {
      let source = mirror.clone();
      let seed = seed.clone();
      let store = store.clone();
      let queue = queue.clone();
      workers.spawn(async move {
        while let Some(lease) = queue.claim_cdn().await {
          let (bytes, proof) = source.fetch(&seed, lease.index, &queue.progress).await?;
          store.commit(lease, bytes, proof).await?;
        }
        Ok::<_, TransferError>(())
      });
    }
    while let Some(result) = workers.join_next().await {
      match result? {
        Ok(()) => {}
        Err(error @ TransferError::Cdn(_)) => last = error,
        Err(error) => return Err(error),
      }
    }
    if queue.complete() {
      return Ok(());
    }
  }
  if store.complete().await? {
    Ok(())
  } else {
    Err(last)
  }
}
