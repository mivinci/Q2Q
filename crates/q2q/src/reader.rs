//! A single verified read cursor over a bounded, seek-driven download window.

use crate::client::Client;
use crate::error::TransferError;
use crate::progress::TransferProgress;
use q2q_core::seed::Seed;
use std::future::{Future, IntoFuture};
use std::io::{self, SeekFrom};
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncSeek, ReadBuf};
use tokio::sync::watch;

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
    crate::destination::check_seed(&self.seed)?;
    if self.prefetch > 64 || self.timeout.is_zero() {
      return Err(TransferError::Protocol(
        "prefetch_blocks must be at most 64 and read_timeout must be nonzero".into(),
      ));
    }
    let peer_wait = Duration::from_secs(2).min(self.timeout / 2);
    let engine = crate::download::open(self.client, self.seed, self.path, self.concurrency).await?;
    Ok(VerifiedFile {
      engine,
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
  engine: crate::download::ReadSession,
  position: u64,
  prefetch: usize,
  timeout: Duration,
  peer_wait: Duration,
  cache: Option<(usize, Vec<u8>)>,
  pending: Option<ReadTask>,
}
impl VerifiedFile {
  pub fn len(&self) -> u64 {
    self.engine.seed.info.length
  }
  pub fn is_empty(&self) -> bool {
    self.len() == 0
  }
  pub fn position(&self) -> u64 {
    self.position
  }
  pub fn progress(&self) -> TransferProgress {
    *self.engine.queue.progress.borrow()
  }
  pub fn subscribe_progress(&self) -> watch::Receiver<TransferProgress> {
    self.engine.queue.progress.subscribe()
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
    let index = (this.position / u64::from(this.engine.seed.info.block_size)) as usize;
    if this.cache.as_ref().is_none_or(|(i, _)| *i != index) {
      if this.pending.is_none() {
        this
          .engine
          .queue
          .focus(index, this.prefetch, this.peer_wait, true);
        let queue = this.engine.queue.clone();
        let store = this.engine.store.clone();
        let timeout = this.timeout;
        this.pending = Some(Box::pin(async move {
          tokio::time::timeout(timeout, async move {
            queue.verified(index).await?;
            store
              .read_verified(index)
              .await
              .map_err(|error| match error {
                TransferError::Io(error) => error,
                error => io::Error::other(error),
              })
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
    let offset = (this.position % u64::from(this.engine.seed.info.block_size)) as usize;
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
      this.engine.seed.info.leaf_count()
    } else {
      (this.position / u64::from(this.engine.seed.info.block_size)) as usize
    };
    this
      .engine
      .queue
      .focus(index, this.prefetch, this.peer_wait, false);
    Ok(())
  }
  fn poll_complete(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<u64>> {
    Poll::Ready(Ok(self.position))
  }
}

#[cfg(test)]
#[path = "../tests/reader.rs"]
mod tests;
