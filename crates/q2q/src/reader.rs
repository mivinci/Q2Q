//! A single verified read cursor over a bounded, seek-driven download window.

use crate::client::Client;
use crate::download_store::DownloadStore;
use crate::progress::{self, TransferProgress, TransferState};
use crate::scheduler::WorkQueue;
use crate::transfer::{self, TransferError};
use q2q_core::seed::Seed;
use std::future::{Future, IntoFuture};
use std::io::{self, SeekFrom};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncSeek, ReadBuf};
use tokio::sync::watch;
use tokio::task::{JoinHandle, JoinSet};

type ReadTask = Pin<Box<dyn Future<Output = io::Result<Vec<u8>>> + Send>>;

/// Configuration returned by [`Client::open`]. Await it to open the file.
pub struct OpenOptions<'a> {
  client: &'a Client,
  seed: Seed,
  path: PathBuf,
  concurrency: usize,
  prefetch: usize,
  timeout: Duration,
}
impl<'a> OpenOptions<'a> {
  pub(crate) fn new(client: &'a Client, seed: Seed, path: PathBuf, concurrency: usize) -> Self {
    Self {
      client,
      seed,
      path,
      concurrency,
      prefetch: 8,
      timeout: Duration::from_secs(30),
    }
  }
  /// Blocks fetched ahead of the read cursor (default 8, maximum 64).
  pub fn prefetch_blocks(mut self, count: usize) -> Self {
    self.prefetch = count;
    self
  }
  /// Maximum wait for one missing block, including verification and disk I/O.
  pub fn read_timeout(mut self, timeout: Duration) -> Self {
    self.timeout = timeout;
    self
  }

  async fn start(self) -> Result<VerifiedFile, TransferError> {
    transfer::check_seed(&self.seed)?;
    if self.prefetch > 64 || self.timeout.is_zero() {
      return Err(TransferError::Protocol(
        "prefetch_blocks must be at most 64 and read_timeout must be nonzero".into(),
      ));
    }
    let resume = tokio::fs::try_exists(&self.path).await?;
    let store = transfer::open_destination(&self.seed, &self.path, resume).await?;
    let bits = store
      .lock()
      .map_err(|_| TransferError::Checkpoint("destination lock poisoned".into()))?
      .verified_bitfield();
    let progress = progress::channel(&self.seed, &bits);
    let queue = WorkQueue::demand(&self.seed, &bits, progress);
    let peer_wait = Duration::from_secs(2).min(self.timeout / 2);
    let seed = Arc::new(self.seed);
    let dial = self.client.connect_owned();
    let peer_id = self.client.peer_id().to_string();
    let task_queue = queue.clone();
    let destination = store.clone();
    let task_seed = seed.clone();
    let concurrency = self.concurrency;
    let path = self.path;
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
            q.fail(error.to_string());
          }
        }
      });
      let q = task_queue.clone();
      tasks.spawn(async move {
        cdn(task_seed, destination, q, concurrency).await;
      });
      while let Some(result) = tasks.join_next().await {
        if let Err(error) = result {
          task_queue.fail(error.to_string());
          return;
        }
      }
      if task_queue.complete() {
        match transfer::sync_destination(&path).await {
          Ok(()) => progress::finish(&task_queue.progress, TransferState::Completed),
          Err(error) => task_queue.fail(error.to_string()),
        }
      }
    });
    Ok(VerifiedFile {
      seed,
      store,
      queue,
      driver,
      position: 0,
      prefetch: self.prefetch,
      timeout: self.timeout,
      peer_wait,
      cache: None,
      pending: None,
    })
  }
}
impl<'a> IntoFuture for OpenOptions<'a> {
  type Output = Result<VerifiedFile, TransferError>;
  type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;
  fn into_future(self) -> Self::IntoFuture {
    Box::pin(self.start())
  }
}

/// A single cursor implementing Tokio `AsyncRead` and `AsyncSeek`.
/// Missing blocks are awaited; sparse destination bytes are never exposed.
/// Dropping this handle cancels network work and preserves verified receipts.
pub struct VerifiedFile {
  seed: Arc<Seed>,
  store: Arc<Mutex<DownloadStore>>,
  queue: Arc<WorkQueue>,
  driver: JoinHandle<()>,
  position: u64,
  prefetch: usize,
  timeout: Duration,
  peer_wait: Duration,
  cache: Option<(usize, Vec<u8>)>,
  pending: Option<ReadTask>,
}
impl VerifiedFile {
  pub fn len(&self) -> u64 {
    self.seed.info.length
  }
  pub fn is_empty(&self) -> bool {
    self.len() == 0
  }
  pub fn position(&self) -> u64 {
    self.position
  }
  pub fn progress(&self) -> TransferProgress {
    *self.queue.progress.borrow()
  }
  pub fn subscribe_progress(&self) -> watch::Receiver<TransferProgress> {
    self.queue.progress.subscribe()
  }
}
impl AsyncRead for VerifiedFile {
  fn poll_read(
    self: Pin<&mut Self>,
    cx: &mut Context<'_>,
    buffer: &mut ReadBuf<'_>,
  ) -> Poll<io::Result<()>> {
    let this = self.get_mut();
    if buffer.remaining() == 0 || this.position >= this.len() {
      return Poll::Ready(Ok(()));
    }
    let index = (this.position / u64::from(this.seed.info.block_size)) as usize;
    if this.cache.as_ref().is_none_or(|(i, _)| *i != index) {
      if this.pending.is_none() {
        this.queue.focus(index, this.prefetch, this.peer_wait, true);
        let queue = this.queue.clone();
        let store = this.store.clone();
        let timeout = this.timeout;
        this.pending = Some(Box::pin(async move {
          tokio::time::timeout(timeout, async move {
            queue.verified(index).await?;
            tokio::task::spawn_blocking(move || {
              let store = store
                .lock()
                .map_err(|_| io::Error::other("destination lock poisoned"))?;
              if store.state(index).map_err(io::Error::other)?
                != q2q_core::block::BlockState::Verified
              {
                return Err(io::Error::other("block is not verified"));
              }
              let bytes = store.read_block(index).map_err(io::Error::other)?;
              if store.cache().leaf(index) != Some(&q2q_core::merkle::leaf_hash(&bytes)) {
                return Err(io::Error::new(
                  io::ErrorKind::InvalidData,
                  "verified destination block changed",
                ));
              }
              Ok(bytes)
            })
            .await
            .map_err(io::Error::other)?
          })
          .await
          .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "verified block read timed out"))?
        }));
      }
      match this.pending.as_mut().expect("read task").as_mut().poll(cx) {
        Poll::Pending => return Poll::Pending,
        Poll::Ready(result) => {
          this.pending = None;
          match result {
            Ok(bytes) => this.cache = Some((index, bytes)),
            Err(error) => return Poll::Ready(Err(error)),
          }
        }
      }
    }
    let bytes = &this.cache.as_ref().expect("verified cache").1;
    let offset = (this.position % u64::from(this.seed.info.block_size)) as usize;
    let count = buffer.remaining().min(bytes.len() - offset);
    buffer.put_slice(&bytes[offset..offset + count]);
    this.position += count as u64;
    Poll::Ready(Ok(()))
  }
}
impl AsyncSeek for VerifiedFile {
  fn start_seek(self: Pin<&mut Self>, seek: SeekFrom) -> io::Result<()> {
    let this = self.get_mut();
    let position = match seek {
      SeekFrom::Start(n) => i128::from(n),
      SeekFrom::Current(n) => i128::from(this.position) + i128::from(n),
      SeekFrom::End(n) => i128::from(this.len()) + i128::from(n),
    };
    this.position = u64::try_from(position)
      .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "seek offset outside u64 range"))?;
    this.pending = None;
    this.cache = None;
    let index = if this.position >= this.len() {
      this.seed.info.leaf_count()
    } else {
      (this.position / u64::from(this.seed.info.block_size)) as usize
    };
    this
      .queue
      .focus(index, this.prefetch, this.peer_wait, false);
    Ok(())
  }
  fn poll_complete(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<u64>> {
    Poll::Ready(Ok(self.position))
  }
}
impl Drop for VerifiedFile {
  fn drop(&mut self) {
    self.driver.abort();
    progress::finish(&self.queue.progress, TransferState::Cancelled);
  }
}

async fn peers(
  connect: impl Future<
    Output = Result<Arc<crate::control::ControlSession>, crate::control::ControlError>,
  >,
  peer_id: String,
  seed: Arc<Seed>,
  store: Arc<Mutex<DownloadStore>>,
  queue: Arc<WorkQueue>,
  concurrency: usize,
) -> Result<(), TransferError> {
  if queue.complete() {
    return Ok(());
  }
  let discovery = async {
    let session = connect.await?;
    let response = session.query(&seed.info_hash()).await?;
    let mut seen = std::collections::HashSet::new();
    let targets: Vec<_> = response
      .peers
      .into_iter()
      .filter(|p| p.complete && p.peer_id != peer_id && seen.insert(p.peer_id.clone()))
      .collect();
    Ok::<_, TransferError>((session, targets))
  };
  let (session, targets) = tokio::select! {
    _ = queue.finished() => return Ok(()),
    result = tokio::time::timeout(Duration::from_secs(15), discovery) => result.map_err(|_| TransferError::Timeout)??,
  };
  if targets.is_empty() {
    return Err(TransferError::NoPeers);
  }
  let manager = async {
    let mut focus = queue.subscribe_focus();
    loop {
      queue.peer_status(false, None);
      let mut workers = JoinSet::new();
      let mut candidates = targets.iter().cloned();
      loop {
        while workers.len() < concurrency && !queue.complete() {
          let Some(target) = candidates.next() else {
            break;
          };
          let session = session.clone();
          let seed = seed.clone();
          let queue = queue.clone();
          let store = store.clone();
          workers.spawn(async move {
            let peer = tokio::select! {
              _ = queue.finished() => return Ok(()),
              result = tokio::time::timeout(Duration::from_secs(45), session.dial_peer(&target, seed.info_hash())) => result.map_err(|_| TransferError::Timeout)??,
            };
            peer.download_scheduled(&seed, store, queue).await
          });
        }
        let Some(result) = workers.join_next().await else {
          break;
        };
        if let Err(error) = result?
          && !error.peer_retryable()
        {
          return Err(error);
        }
      }
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
    error = crate::publication::keep_online(&session) => Err(TransferError::Control(error)),
  }
}

async fn cdn(
  seed: Arc<Seed>,
  store: Arc<Mutex<DownloadStore>>,
  queue: Arc<WorkQueue>,
  concurrency: usize,
) {
  if queue.complete() {
    return;
  }
  if seed.cdn_list.is_empty() {
    queue.cdn_status(Err(
      "no CDN sources configured and peers unavailable".into(),
    ));
    return;
  }
  tokio::select! { _ = queue.finished() => return, _ = queue.cdn_needed() => {} }
  if queue.complete() {
    return;
  }
  let source = match crate::cdn::Source::load(&seed, &queue.progress).await {
    Ok(source) => Arc::new(source),
    Err(error) => {
      queue.cdn_status(Err(error.to_string()));
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
            queue.block_failed(index, error.to_string());
            drop(lease);
            continue;
          }
        };
        let destination = store.clone();
        let root = seed.info.root;
        let guard = lease.disk_task();
        tokio::task::spawn_blocking(move || {
          let _guard = guard;
          let mut store = destination
            .lock()
            .map_err(|_| TransferError::Checkpoint("destination lock poisoned".into()))?;
          store.commit(index, &bytes, proof, &root)?;
          lease.commit();
          Ok::<_, TransferError>(())
        })
        .await??;
      }
      Ok::<_, TransferError>(())
    });
  }
  while let Some(result) = tasks.join_next().await {
    match result {
      Ok(Ok(())) => {}
      Ok(Err(error)) => {
        queue.fail(error.to_string());
        return;
      }
      Err(error) => {
        queue.fail(error.to_string());
        return;
      }
    }
  }
}

#[cfg(test)]
#[path = "../tests/reader.rs"]
mod tests;
