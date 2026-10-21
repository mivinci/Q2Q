use crate::{Checksum, Error, Executor, ExecutorBuilder, Response, store::Store};
use std::{
  collections::VecDeque,
  future::Future,
  path::PathBuf,
  sync::{Arc, Mutex},
  time::Duration,
};
use tokio::{
  sync::{Semaphore, watch},
  task::{JoinHandle, JoinSet},
};

#[derive(Clone)]
pub(crate) struct Cancel(watch::Sender<bool>);
impl Cancel {
  fn new() -> Self {
    Self(watch::channel(false).0)
  }
  fn cancel(&self) {
    self.0.send_replace(true);
  }
  pub(crate) fn check(&self) -> Result<(), Error> {
    if *self.0.borrow() {
      Err(Error::Cancelled)
    } else {
      Ok(())
    }
  }
  async fn cancelled(&self) {
    let mut r = self.0.subscribe();
    loop {
      if *r.borrow_and_update() {
        return;
      }
      if r.changed().await.is_err() {
        return;
      }
    }
  }
}
/// File transfer state. Only Completed means a verified, published destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownloadState {
  Queued,
  Downloading,
  Verifying,
  Publishing,
  Completed,
  Failed,
  Cancelled,
}
impl DownloadState {
  pub fn is_terminal(self) -> bool {
    matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
  }
}
#[derive(Clone, Copy, Debug)]
pub struct Progress {
  pub total_bytes: u64,
  pub staged_bytes: u64,
  pub received_bytes: u64,
  pub verification_bytes: u64,
  pub active_requests: usize,
  pub state: DownloadState,
}
/// A single file; URLs are intentionally not included in Debug output.
#[derive(Clone)]
pub struct DownloadRequest {
  path: PathBuf,
  mirrors: Vec<String>,
  size: Option<u64>,
  checksum: Option<Checksum>,
}
impl DownloadRequest {
  pub fn new(path: impl Into<PathBuf>) -> Self {
    Self {
      path: path.into(),
      mirrors: Vec::new(),
      size: None,
      checksum: None,
    }
  }
  pub fn mirrors<I, S>(mut self, urls: I) -> Self
  where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
  {
    self.mirrors = urls.into_iter().map(|s| s.as_ref().into()).collect();
    self
  }
  pub fn expected_size(mut self, n: u64) -> Self {
    self.size = Some(n);
    self
  }
  pub fn checksum(mut self, c: Checksum) -> Self {
    self.checksum = Some(c);
    self
  }
}
#[derive(Clone)]
pub(crate) struct Spec {
  pub path: PathBuf,
  pub size: u64,
  pub checksum: Checksum,
  mirrors: Vec<String>,
}
impl TryFrom<DownloadRequest> for Spec {
  type Error = Error;
  fn try_from(r: DownloadRequest) -> Result<Self, Error> {
    let size = r.size.ok_or(Error::Invalid("expected_size is required"))?;
    let checksum = r.checksum.ok_or(Error::Invalid("checksum is required"))?;
    if r.path.file_name().is_none() || r.mirrors.len() > 32 {
      return Err(Error::Invalid("destination or mirror count"));
    }
    for url in &r.mirrors {
      crate::executor::parse_url(url)?;
    }
    if size > 0 && r.mirrors.is_empty() {
      return Err(Error::Invalid("at least one mirror is required"));
    }
    Ok(Self {
      path: r.path,
      size,
      checksum,
      mirrors: r.mirrors,
    })
  }
}
#[derive(Clone)]
pub struct Downloader {
  inner: Arc<Inner>,
}
struct Inner {
  http: Executor,
  files: Arc<Semaphore>,
  disk: Arc<Semaphore>,
  chunk_size: u64,
  threshold: u64,
  ranges: usize,
  retries: usize,
  retry_delay: Duration,
}
pub struct DownloaderBuilder {
  http: ExecutorBuilder,
  files: usize,
  disk: usize,
  chunk_size: u64,
  threshold: u64,
  ranges: usize,
  retries: usize,
  retry_delay: Duration,
}
impl Default for DownloaderBuilder {
  fn default() -> Self {
    Self {
      http: Executor::builder(),
      files: 8,
      disk: 2,
      chunk_size: 1024 * 1024,
      threshold: 8 * 1024 * 1024,
      ranges: 4,
      retries: 2,
      retry_delay: Duration::from_millis(250),
    }
  }
}
impl DownloaderBuilder {
  pub fn max_active_files(mut self, n: usize) -> Self {
    self.files = n;
    self
  }
  pub fn max_requests(mut self, n: usize) -> Self {
    self.http = self.http.max_requests(n);
    self
  }
  pub fn max_requests_per_host(mut self, n: usize) -> Self {
    self.http = self.http.max_requests_per_host(n);
    self
  }
  pub fn max_disk_tasks(mut self, n: usize) -> Self {
    self.disk = n;
    self
  }
  pub fn max_ranges_per_file(mut self, n: usize) -> Self {
    self.ranges = n;
    self
  }
  pub fn chunk_size(mut self, n: u64) -> Self {
    self.chunk_size = n;
    self
  }
  pub fn range_threshold(mut self, n: u64) -> Self {
    self.threshold = n;
    self
  }
  pub fn retries(mut self, n: usize) -> Self {
    self.retries = n;
    self
  }
  pub fn retry_delay(mut self, d: Duration) -> Self {
    self.retry_delay = d;
    self
  }
  pub fn timeout(mut self, d: Duration) -> Self {
    self.http = self.http.timeout(d);
    self
  }
  pub fn build(self) -> Result<Downloader, Error> {
    if self.files == 0
      || self.files > Semaphore::MAX_PERMITS
      || self.disk == 0
      || self.disk > Semaphore::MAX_PERMITS
      || !(1..=16).contains(&self.ranges)
      || self.chunk_size == 0
      || self.chunk_size > 8 * 1024 * 1024
      || self.retries > 16
      || self.retry_delay > Duration::from_secs(30)
    {
      return Err(Error::Invalid("invalid file, disk, range or retry budget"));
    }
    Ok(Downloader {
      inner: Arc::new(Inner {
        http: self.http.build()?,
        files: Arc::new(Semaphore::new(self.files)),
        disk: Arc::new(Semaphore::new(self.disk)),
        chunk_size: self.chunk_size,
        threshold: self.threshold,
        ranges: self.ranges,
        retries: self.retries,
        retry_delay: self.retry_delay,
      }),
    })
  }
}
impl Downloader {
  pub fn builder() -> DownloaderBuilder {
    DownloaderBuilder::default()
  }
  /// Share this executor for bounded metadata requests as well as file downloads.
  pub fn executor(&self) -> Executor {
    self.inner.http.clone()
  }
  pub async fn download(&self, request: DownloadRequest) -> Result<Task, Error> {
    self.start(request, false).await
  }
  pub async fn resume(&self, request: DownloadRequest) -> Result<Task, Error> {
    self.start(request, true).await
  }
  async fn start(&self, request: DownloadRequest, resume: bool) -> Result<Task, Error> {
    let spec = Spec::try_from(request)?;
    if spec.size.div_ceil(self.inner.chunk_size) > 65536 {
      return Err(Error::Invalid("file exceeds 65536 configured ranges"));
    }
    // Admission is bounded before spawn. Dropping an admission future leaves no
    // background task or target lock behind.
    let permit = self
      .inner
      .files
      .clone()
      .acquire_owned()
      .await
      .map_err(|_| Error::Cancelled)?;
    let progress = watch::channel(Progress {
      total_bytes: spec.size,
      staged_bytes: 0,
      received_bytes: 0,
      verification_bytes: 0,
      active_requests: 0,
      state: DownloadState::Queued,
    })
    .0;
    let cancel = Cancel::new();
    let operation_cancel = cancel.clone();
    let updates = progress.clone();
    let downloader = self.clone();
    let task = tokio::spawn(async move {
      let _permit = permit;
      let result = downloader
        .run(spec, resume, &operation_cancel, &updates)
        .await;
      updates.send_modify(|p| {
        p.active_requests = 0;
        if result.is_ok() {
          p.staged_bytes = p.total_bytes;
        }
        p.state = match &result {
          Ok(_) => DownloadState::Completed,
          Err(Error::Cancelled) => DownloadState::Cancelled,
          Err(_) => DownloadState::Failed,
        };
      });
      result
    });
    Ok(Task {
      task: Some(task),
      cancel,
      progress,
    })
  }
  async fn run(
    &self,
    spec: Spec,
    resume: bool,
    cancel: &Cancel,
    progress: &watch::Sender<Progress>,
  ) -> Result<PathBuf, Error> {
    cancel.check()?;
    let store = Store::open(
      spec.clone(),
      self.inner.chunk_size,
      resume,
      self.inner.disk.clone(),
    )
    .await?;
    cancel.check()?;
    let (published, done, staged, _) = store.snapshot().await?;
    progress.send_modify(|p| {
      p.staged_bytes = staged;
      p.state = DownloadState::Downloading;
    });
    if published || done.iter().all(|d| *d) {
      progress.send_modify(|p| p.state = DownloadState::Verifying);
      match store.verify(cancel.clone(), progress.clone()).await {
        Ok(()) => return store.publish(cancel.clone(), progress.clone()).await,
        Err(Error::Integrity) if !published && spec.size > 0 => store.reset().await?,
        Err(e) => return Err(e),
      }
    }
    {
      let mut last = Error::Invalid("no file source");
      'mirrors: for source in &spec.mirrors {
        store.source(source).await?;
        for attempt in 0..=self.inner.retries {
          cancel.check()?;
          let result = self.transfer(&spec, source, &store, cancel, progress).await;
          match result {
            Ok(()) => {
              progress.send_modify(|p| p.state = DownloadState::Verifying);
              match store.verify(cancel.clone(), progress.clone()).await {
                Ok(()) => return store.publish(cancel.clone(), progress.clone()).await,
                Err(Error::Integrity) => {
                  store.reset().await?;
                  progress.send_modify(|p| {
                    p.staged_bytes = 0;
                    p.verification_bytes = 0;
                  });
                  last = Error::Integrity;
                  if attempt < self.inner.retries {
                    continue;
                  }
                  continue 'mirrors;
                }
                Err(e) => return Err(e),
              }
            }
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(
              e @ (Error::Io(_)
              | Error::Checkpoint(_)
              | Error::Busy
              | Error::Task(_)
              | Error::Invalid(_)),
            ) => return Err(e),
            Err(e) => {
              let retry = e.retryable() && attempt < self.inner.retries;
              let server_delay = match &e {
                Error::Status { retry_after, .. } => *retry_after,
                _ => None,
              };
              last = e;
              if !retry {
                break;
              }
              let delay = server_delay
                .unwrap_or(self.inner.retry_delay.saturating_mul(1 << attempt.min(7)))
                .min(Duration::from_secs(30));
              tokio::select! {_=cancel.cancelled()=>return Err(Error::Cancelled),_=tokio::time::sleep(delay)=>{}}
              store.source(source).await?;
            }
          }
        }
      }
      Err(last)
    }
  }

  async fn transfer(
    &self,
    spec: &Spec,
    source: &str,
    store: &Store,
    cancel: &Cancel,
    progress: &watch::Sender<Progress>,
  ) -> Result<(), Error> {
    progress.send_modify(|p| {
      p.state = DownloadState::Downloading;
      p.verification_bytes = 0;
    });
    if spec.size < self.inner.threshold {
      return self.full(spec, source, store, cancel, progress).await;
    }
    let (_, done, staged, validator) = store.snapshot().await?;
    progress.send_modify(|p| p.staged_bytes = staged);
    let Some(first) = done.iter().position(|d| !d) else {
      return Ok(());
    };
    let range = self.range(spec, first);
    let mut request = self.inner.http.get_range(source, range.clone(), spec.size);
    if let Some(etag) = validator {
      request = request.if_range(etag);
    }
    let local = Cancel::new();
    let active = Active::new(progress.clone());
    let response = network(request.send(), cancel, &local).await;
    let response = match response {
      Err(Error::RangeUnsupported) => {
        store.reset().await?;
        drop(active);
        return self.full(spec, source, store, cancel, progress).await;
      }
      // A changed validator invalidates old staging; restart with a fresh probe.
      Err(Error::SourceChanged) => {
        store.reset().await?;
        drop(active);
        return self.full(spec, source, store, cancel, progress).await;
      }
      other => other?,
    };
    let etag = response.etag().map(str::to_owned);
    store.validator(etag.clone()).await?;
    let bytes = collect(response, cancel, &local, progress).await?;
    drop(active);
    let staged = store
      .write(range.start, bytes, Some(first), cancel.clone())
      .await?;
    progress.send_modify(|p| p.staged_bytes = staged);
    let (_, done, _, _) = store.snapshot().await?;
    let pending = Arc::new(Mutex::new(
      done
        .into_iter()
        .enumerate()
        .filter_map(|(i, d)| (!d).then_some(i))
        .collect::<VecDeque<_>>(),
    ));
    let mut workers = JoinSet::new();
    for _ in 0..self.inner.ranges {
      let pending = pending.clone();
      let source = source.to_owned();
      let spec = spec.clone();
      let store = store.clone();
      let cancel = cancel.clone();
      let local = local.clone();
      let http = self.inner.http.clone();
      let updates = progress.clone();
      let etag = etag.clone();
      let chunk_size = self.inner.chunk_size;
      workers.spawn(async move {
        loop {
          cancel.check()?;
          local.check()?;
          let index = pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front();
          let Some(index) = index else {
            return Ok::<_, Error>(());
          };
          let start = index as u64 * chunk_size;
          let end = (start + chunk_size).min(spec.size);
          let mut request = http.get_range(&source, start..end, spec.size);
          if let Some(etag) = &etag {
            request = request.if_range(etag);
          }
          let _active = Active::new(updates.clone());
          let response = network(request.send(), &cancel, &local).await?;
          let bytes = collect(response, &cancel, &local, &updates).await?;
          let staged = store
            .write(start, bytes, Some(index), cancel.clone())
            .await?;
          updates.send_modify(|p| p.staged_bytes = p.staged_bytes.max(staged));
        }
      });
    }
    let mut error = None;
    // Do not abort workers that may be awaiting blocking disk commits. Cancel
    // their network work and drain them so wait also waits for target unlock.
    while let Some(result) = workers.join_next().await {
      let result = result.map_err(Error::from).and_then(|r| r);
      if let Err(e) = result
        && error.is_none()
      {
        error = Some(e);
        local.cancel();
      }
    }
    match error {
      Some(Error::RangeUnsupported | Error::SourceChanged) => {
        store.reset().await?;
        self.full(spec, source, store, cancel, progress).await
      }
      Some(e) => Err(e),
      None => Ok(()),
    }
  }
  fn range(&self, spec: &Spec, index: usize) -> std::ops::Range<u64> {
    let start = index as u64 * self.inner.chunk_size;
    start..(start + self.inner.chunk_size).min(spec.size)
  }
  async fn full(
    &self,
    spec: &Spec,
    source: &str,
    store: &Store,
    cancel: &Cancel,
    progress: &watch::Sender<Progress>,
  ) -> Result<(), Error> {
    store.reset().await?;
    progress.send_modify(|p| p.staged_bytes = 0);
    let local = Cancel::new();
    let _active = Active::new(progress.clone());
    let mut response = network(
      self.inner.http.get(source).expected_size(spec.size).send(),
      cancel,
      &local,
    )
    .await?;
    let mut offset = 0;
    while let Some(bytes) = network(response.chunk(), cancel, &local).await? {
      let len = bytes.len() as u64;
      received(progress, len);
      store.write(offset, bytes, None, cancel.clone()).await?;
      offset += len;
    }
    let staged = store.full_complete().await?;
    progress.send_modify(|p| p.staged_bytes = staged);
    Ok(())
  }
}
async fn collect(
  mut response: Response,
  cancel: &Cancel,
  local: &Cancel,
  progress: &watch::Sender<Progress>,
) -> Result<bytes::Bytes, Error> {
  let mut bytes = bytes::BytesMut::new();
  while let Some(chunk) = network(response.chunk(), cancel, local).await? {
    received(progress, chunk.len() as u64);
    bytes.extend_from_slice(&chunk);
  }
  Ok(bytes.freeze())
}
async fn network<T>(
  future: impl Future<Output = Result<T, Error>>,
  cancel: &Cancel,
  local: &Cancel,
) -> Result<T, Error> {
  tokio::select! {biased;_=cancel.cancelled()=>Err(Error::Cancelled),_=local.cancelled()=>Err(Error::Cancelled),r=future=>r}
}
fn received(progress: &watch::Sender<Progress>, n: u64) {
  progress.send_modify(|p| p.received_bytes = p.received_bytes.saturating_add(n));
}
struct Active(watch::Sender<Progress>);
impl Active {
  fn new(p: watch::Sender<Progress>) -> Self {
    p.send_modify(|p| p.active_requests += 1);
    Self(p)
  }
}
impl Drop for Active {
  fn drop(&mut self) {
    self.0.send_modify(|p| p.active_requests -= 1);
  }
}
/// Dropping cancels; wait returns only after workers and blocking I/O drain.
pub struct Task {
  task: Option<JoinHandle<Result<PathBuf, Error>>>,
  cancel: Cancel,
  progress: watch::Sender<Progress>,
}
impl Task {
  pub fn progress(&self) -> Progress {
    *self.progress.borrow()
  }
  pub fn subscribe_progress(&self) -> watch::Receiver<Progress> {
    self.progress.subscribe()
  }
  pub fn cancel(&self) {
    self.cancel.cancel();
  }
  pub async fn wait(mut self) -> Result<PathBuf, Error> {
    let result = self.task.as_mut().expect("owned task").await;
    self.task.take();
    result?
  }
}
impl Drop for Task {
  fn drop(&mut self) {
    if self.task.is_some() {
      self.cancel.cancel();
    }
  }
}
