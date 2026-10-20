//! Download orchestration shared by full-file transfers and seekable readers.
use crate::block_queue::WorkQueue;
use crate::client::Client;
use crate::control::{ControlError, ControlSession};
use crate::destination::Destination;
use crate::error::TransferError;
use crate::progress::{self, TransferState};
use crate::transfer::Transfer;
use q2q_primitives::seed::Seed;
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
  start_with_store_inner(client, seed, path, resume, concurrency, None, false).await
}

pub(crate) async fn start_with_track_discovery(
  client: &Client,
  seed: &Seed,
  path: PathBuf,
  resume: bool,
  concurrency: usize,
) -> Result<Transfer, TransferError> {
  start_with_store_inner(client, seed, path, resume, concurrency, None, true).await
}

pub(crate) async fn start_with_store(
  client: &Client,
  seed: &Seed,
  path: PathBuf,
  resume: bool,
  concurrency: usize,
  supplied_store: Option<Destination>,
) -> Result<Transfer, TransferError> {
  start_with_store_inner(
    client,
    seed,
    path,
    resume,
    concurrency,
    supplied_store,
    false,
  )
  .await
}

pub(crate) async fn start_with_store_and_track_discovery(
  client: &Client,
  seed: &Seed,
  path: PathBuf,
  resume: bool,
  concurrency: usize,
  supplied_store: Destination,
) -> Result<Transfer, TransferError> {
  start_with_store_inner(
    client,
    seed,
    path,
    resume,
    concurrency,
    Some(supplied_store),
    true,
  )
  .await
}

fn start_with_store_inner<'a>(
  client: &'a Client,
  seed: &'a Seed,
  path: PathBuf,
  resume: bool,
  concurrency: usize,
  supplied_store: Option<Destination>,
  require_track_discovery: bool,
) -> impl Future<Output = Result<Transfer, TransferError>> + Send + 'a {
  // Keep discovery state on the heap so callers composing multiple download
  // modes do not multiply its stack footprint (notably on Windows).
  Box::pin(async move {
    crate::destination::check_seed(seed)?;
    let checkpoint_directory = client.storage_dir().join("checkpoints");
    let supplied = supplied_store.is_some();
    let initial_store = if let Some(store) = supplied_store {
      Some(store)
    } else if resume {
      Some(Destination::open_in(seed, &path, true, &checkpoint_directory).await?)
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
      let store = initial_store.expect("resumed destination");
      let mut guard = crate::destination::DownloadGuard::new(store.clone());
      return Ok(Transfer::start(progress, async move {
        store.finish().await?;
        guard.finish();
        Ok(path)
      }));
    }
    if !supplied && !resume && tokio::fs::try_exists(&path).await? {
      return Err(
        std::io::Error::new(
          std::io::ErrorKind::AlreadyExists,
          "download destination already exists",
        )
        .into(),
      );
    }
    if seed.info.length == 0 {
      let store = match initial_store {
        Some(store) => store,
        None => Destination::open_in(seed, &path, false, &checkpoint_directory).await?,
      };
      let mut guard = crate::destination::DownloadGuard::new(store.clone());
      return Ok(Transfer::start(progress, async move {
        store.finish().await?;
        guard.finish();
        Ok(path)
      }));
    }
    // A failed control plane must not prevent CDN-only cold start. Bound the
    // complete discovery path, including Punch Binding setup, before fallback.
    let discovery = discover(
      client.connect_owned(),
      client.peer_id().to_string(),
      seed,
      client.peer_query_limit(),
    )
    .await;
    let discovery = match discovery {
      Err(error) if require_track_discovery => return Err(error),
      Err(error) if seed.cdn_list.is_empty() => return Err(error),
      Ok((_, targets)) if targets.is_empty() && require_track_discovery => {
        return Err(TransferError::NoPeers);
      }
      Ok((_, targets)) if targets.is_empty() && !seed.cdn_list.is_empty() => {
        Err(TransferError::NoPeers)
      }
      result => result,
    };
    let query_limit = client.peer_query_limit();
    let seed = seed.clone();
    let updates = progress.clone();
    let store = match initial_store {
      Some(store) => store,
      None => Destination::open_in(&seed, &path, false, &checkpoint_directory).await?,
    };
    let mut guard = crate::destination::DownloadGuard::new(store.clone());
    let peers = crate::download_progress::Progress::new();
    let peer_updates = peers.clone();
    let accept_invalid_certs = client.accept_invalid_certs();
    Ok(Transfer::observed(progress, peers, async move {
      let activity = crate::scheduler::TaskScope::new();
      let p2p = match discovery {
        Ok((session, targets)) => {
          let bits = store.bitfield().await?;
          let queue = WorkQueue::observed(
            &seed,
            &bits,
            updates.clone(),
            activity.clone(),
            peer_updates,
          );
          let attempt = crate::discovery::Peers {
            session: session.clone(),
            candidates: targets,
            concurrency,
            query_limit,
          }
          .run(Arc::new(seed.clone()), store.clone(), queue, false);
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
        cdn_all(
          seed,
          store.clone(),
          concurrency,
          updates,
          accept_invalid_certs,
        )
        .await?;
      }
      store.finish().await?;
      guard.finish();
      Ok(path)
    }))
  })
}

pub(crate) struct ReadSession {
  pub(crate) seed: Arc<Seed>,
  pub(crate) store: Destination,
  pub(crate) queue: Arc<WorkQueue>,
  driver: JoinHandle<()>,
}
impl Drop for ReadSession {
  fn drop(&mut self) {
    self.queue.peers.close();
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
  let checkpoint_directory = client.storage_dir().join("checkpoints");
  let store = Destination::open_in(&seed, &path, resume, &checkpoint_directory).await?;
  let bits = store.bitfield().await?;
  let progress = progress::channel(&seed, &bits);
  let queue = WorkQueue::demand(&seed, &bits, progress);
  let seed = Arc::new(seed);
  let dial = client.connect_owned();
  let peer_id = client.peer_id().to_string();
  let query_limit = client.peer_query_limit();
  let task_queue = queue.clone();
  let accept_invalid_certs = client.accept_invalid_certs();
  let destination = store.clone();
  let task_seed = seed.clone();
  let finishing = store.clone();
  let mut guard = crate::destination::DownloadGuard::new(store.clone());
  let observations = queue.peers.clone();
  let driver = tokio::spawn(async move {
    let operation = async move {
      let mut tasks = JoinSet::new();
      let q = task_queue.clone();
      let s = task_seed.clone();
      let disk = destination.clone();
      tasks.spawn(async move {
        let outcome = peers(dial, peer_id, s, disk, q.clone(), concurrency, query_limit).await;
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
        cdn_window(task_seed, destination, q, concurrency, accept_invalid_certs).await;
      });
      while let Some(result) = tasks.join_next().await {
        if let Err(error) = result {
          task_queue.fail(error.into());
          return;
        }
      }
      if task_queue.complete() {
        match finishing.finish().await {
          Ok(()) => {
            guard.finish();
            task_queue.peers.close();
            progress::finish(&task_queue.progress, TransferState::Completed);
          }
          Err(error) => task_queue.fail(error),
        }
      }
    };
    tokio::select! {
      _ = operation => {},
      _ = observations.coalesce() => unreachable!("coalescing loop does not end"),
    }
    observations.close();
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
  query_limit: usize,
) -> Result<(Arc<ControlSession>, Vec<q2q_track::wire::PeerWire>), TransferError> {
  tokio::time::timeout(Duration::from_secs(15), async {
    let session = connect.await?;
    let response = session
      .query_with_options(
        &seed.info_hash(),
        crate::control::QueryOptions {
          limit: Some(query_limit),
          cursor: None,
        },
      )
      .await?;
    let mut seen = std::collections::HashSet::new();
    let targets: Vec<_> = response
      .peers
      .into_iter()
      .filter(|p| p.peer_id != peer_id && seen.insert(p.peer_id.clone()))
      .collect();
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
  query_limit: usize,
) -> Result<(), TransferError> {
  if queue.complete() {
    return Ok(());
  }
  let (session, targets) = tokio::select! {
    _ = queue.finished() => return Ok(()),
    result = discover(connect, peer_id, &seed, query_limit) => result?,
  };
  let peers = crate::discovery::Peers {
    session: session.clone(),
    candidates: targets,
    concurrency,
    query_limit,
  };
  tokio::select! {
    result = peers.run(seed, store, queue, true) => result,
    error = session.keep_online() => Err(TransferError::Control(error)),
  }
}

pub(crate) async fn cdn_window(
  seed: Arc<Seed>,
  store: Destination,
  queue: Arc<WorkQueue>,
  concurrency: usize,
  accept_invalid_certs: bool,
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
  let source = match crate::cdn::Source::load(&seed, &queue.progress, accept_invalid_certs).await {
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
  accept_invalid_certs: bool,
) -> Result<(), TransferError> {
  let bits = store.bitfield().await?;
  let queue = WorkQueue::new(&seed, &bits, progress.clone());
  if queue.complete() {
    return Ok(());
  }
  let source = crate::cdn::Source::load(&seed, &progress, accept_invalid_certs).await?;
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
