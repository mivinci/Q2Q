//! Separate from Q2Q Merkle receipts: ranges here are staging, not trusted blocks.
use crate::{
  Checksum, Error,
  downloader::{Cancel, DownloadState, Progress, Spec},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
  fs::{self, File, OpenOptions},
  io::{Read, Seek, SeekFrom, Write},
  path::{Path, PathBuf},
  sync::{Arc, Mutex},
};
use tokio::sync::{Semaphore, watch};

const MAX_CHECKPOINT: u64 = 2 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
  version: u32,
  target: [u8; 32],
  length: u64,
  checksum: Checksum,
  chunk_size: u64,
  source: Option<[u8; 32]>,
  etag: Option<String>,
  done: Vec<bool>,
}
struct DiskStore {
  _lock: File,
  file: File,
  target: PathBuf,
  part: PathBuf,
  journal_path: PathBuf,
  journal: Journal,
  published: bool,
  failed: bool,
}
#[derive(Clone)]
pub(crate) struct Store {
  disk: Arc<Mutex<DiskStore>>,
  budget: Arc<Semaphore>,
}
fn sidecar(path: &Path, suffix: &str) -> PathBuf {
  let mut p = path.as_os_str().to_owned();
  p.push(suffix);
  p.into()
}
fn reject_link(path: &Path) -> Result<(), Error> {
  match fs::symlink_metadata(path) {
    Ok(m) if m.file_type().is_symlink() || !m.is_file() => {
      Err(Error::Checkpoint("unexpected non-regular file"))
    }
    Ok(_) => Ok(()),
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
    Err(e) => Err(e.into()),
  }
}
fn sync_directory(path: &Path) -> Result<(), Error> {
  #[cfg(unix)]
  File::open(path)?.sync_all()?;
  #[cfg(not(unix))]
  let _ = path;
  Ok(())
}
impl DiskStore {
  fn open(spec: &Spec, chunk_size: u64, resume: bool) -> Result<Self, Error> {
    let parent = spec
      .path
      .parent()
      .filter(|p| !p.as_os_str().is_empty())
      .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let parent = fs::canonicalize(parent)?;
    let target = parent.join(
      spec
        .path
        .file_name()
        .ok_or(Error::Invalid("destination must name a file"))?,
    );
    let part = sidecar(&target, ".q2q-http.part");
    let journal_path = sidecar(&target, ".q2q-http.resume");
    let lock_path = sidecar(&target, ".q2q-http.lock");
    for p in [&target, &part, &journal_path, &lock_path] {
      reject_link(p)?;
    }
    let lock = OpenOptions::new()
      .read(true)
      .write(true)
      .create(true)
      .truncate(false)
      .open(lock_path)?;
    lock.try_lock().map_err(|e| match e {
      std::fs::TryLockError::WouldBlock => Error::Busy,
      std::fs::TryLockError::Error(e) => Error::Io(e),
    })?;
    let identity: [u8; 32] = Sha256::digest(target.as_os_str().as_encoded_bytes()).into();
    let journal = Journal {
      version: 1,
      target: identity,
      length: spec.size,
      checksum: spec.checksum.clone(),
      chunk_size,
      source: None,
      etag: None,
      done: vec![false; spec.size.div_ceil(chunk_size) as usize],
    };
    if target.try_exists()? {
      if !resume {
        return Err(
          std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "destination already exists",
          )
          .into(),
        );
      }
      let file = File::open(&target)?;
      if file.metadata()?.len() != spec.size {
        return Err(Error::Integrity);
      }
      // Only clean sidecars whose identity belongs to this request. A valid
      // published target must not authorize deleting unrelated staging files.
      if part.try_exists()? || journal_path.try_exists()? {
        let f = File::open(&journal_path)?;
        if f.metadata()?.len() > MAX_CHECKPOINT {
          return Err(Error::Checkpoint("record exceeds size limit"));
        }
        let old: Journal = serde_json::from_reader(f.take(MAX_CHECKPOINT + 1))
          .map_err(|_| Error::Checkpoint("malformed record"))?;
        if old.version != 1
          || old.target != identity
          || old.length != spec.size
          || old.checksum != spec.checksum
        {
          return Err(Error::Checkpoint(
            "published sidecars belong to another request",
          ));
        }
      }
      // Final verification is performed through the same cancellable path as staging.
      return Ok(Self {
        _lock: lock,
        file,
        target,
        part,
        journal_path,
        journal,
        published: true,
        failed: false,
      });
    }
    let (file, journal) = if resume {
      let f = File::open(&journal_path)?;
      if f.metadata()?.len() > MAX_CHECKPOINT {
        return Err(Error::Checkpoint("record exceeds size limit"));
      }
      let old: Journal = serde_json::from_reader(f.take(MAX_CHECKPOINT + 1))
        .map_err(|_| Error::Checkpoint("malformed record"))?;
      if old.version != 1
        || old.target != identity
        || old.length != journal.length
        || old.checksum != journal.checksum
        || old.chunk_size != chunk_size
        || old.done.len() != journal.done.len()
        || old
          .etag
          .as_ref()
          .is_some_and(|e| e.len() > 8192 || !crate::executor::strong_etag(e))
      {
        return Err(Error::Checkpoint("record does not match download request"));
      }
      let file = OpenOptions::new().read(true).write(true).open(&part)?;
      if file.metadata()?.len() != spec.size {
        return Err(Error::Checkpoint("staging length changed"));
      }
      (file, old)
    } else {
      if part.try_exists()? || journal_path.try_exists()? {
        return Err(Error::Checkpoint("staging already exists; use resume"));
      }
      let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&part)?;
      if let Err(e) = file.set_len(spec.size) {
        drop(file);
        let _ = fs::remove_file(&part);
        return Err(e.into());
      }
      (file, journal)
    };
    let store = Self {
      _lock: lock,
      file,
      target,
      part,
      journal_path,
      journal,
      published: false,
      failed: false,
    };
    store.save()?;
    Ok(store)
  }
  fn save(&self) -> Result<(), Error> {
    let parent = self.target.parent().unwrap();
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(temp.as_file_mut(), &self.journal)
      .map_err(|_| Error::Checkpoint("cannot encode record"))?;
    temp.as_file().sync_all()?;
    temp
      .persist(&self.journal_path)
      .map_err(|e| Error::Io(e.error))?;
    sync_directory(parent)
  }
  fn staged(&self) -> u64 {
    self
      .journal
      .done
      .iter()
      .enumerate()
      .filter(|(_, done)| **done)
      .map(|(i, _)| self.block_len(i))
      .sum()
  }
  fn block_len(&self, index: usize) -> u64 {
    self
      .journal
      .chunk_size
      .min(self.journal.length - index as u64 * self.journal.chunk_size)
  }
}
impl Store {
  pub(crate) async fn open(
    spec: Spec,
    chunk_size: u64,
    resume: bool,
    budget: Arc<Semaphore>,
  ) -> Result<Self, Error> {
    let permit = budget
      .clone()
      .acquire_owned()
      .await
      .map_err(|_| Error::Cancelled)?;
    let disk = tokio::task::spawn_blocking(move || {
      let _permit = permit;
      DiskStore::open(&spec, chunk_size, resume)
    })
    .await??;
    Ok(Self {
      disk: Arc::new(Mutex::new(disk)),
      budget,
    })
  }
  // The blocking closure retains both the target lock and its I/O permit even if
  // the awaiting future is dropped. Normal cancellation drains worker futures.
  async fn access<T: Send + 'static>(
    &self,
    f: impl FnOnce(&mut DiskStore) -> Result<T, Error> + Send + 'static,
  ) -> Result<T, Error> {
    let permit = self
      .budget
      .clone()
      .acquire_owned()
      .await
      .map_err(|_| Error::Cancelled)?;
    let disk = self.disk.clone();
    tokio::task::spawn_blocking(move || {
      let _permit = permit;
      let mut d = disk
        .lock()
        .map_err(|_| Error::Checkpoint("storage lock poisoned"))?;
      if d.failed {
        return Err(Error::Checkpoint("previous storage operation failed"));
      }
      let result = f(&mut d);
      if matches!(&result, Err(Error::Io(_) | Error::Checkpoint(_))) {
        d.failed = true;
      }
      result
    })
    .await?
  }
  pub(crate) async fn snapshot(&self) -> Result<(bool, Vec<bool>, u64, Option<String>), Error> {
    self
      .access(|d| {
        Ok((
          d.published,
          d.journal.done.clone(),
          d.staged(),
          d.journal.etag.clone(),
        ))
      })
      .await
  }
  pub(crate) async fn source(&self, source: &str) -> Result<(), Error> {
    let hash: [u8; 32] = Sha256::digest(source.as_bytes()).into();
    self
      .access(move |d| {
        // Without an authenticated block table, old ranges need an unchanged
        // strong validator. The final checksum is required in every case.
        if d.journal.source != Some(hash) || d.journal.etag.is_none() {
          d.journal.done.fill(false);
          d.journal.etag = None;
        }
        d.journal.source = Some(hash);
        d.save()
      })
      .await
  }
  pub(crate) async fn validator(&self, etag: Option<String>) -> Result<(), Error> {
    self
      .access(move |d| {
        if d.journal.etag != etag {
          d.journal.done.fill(false);
        }
        d.journal.etag = etag;
        d.save()
      })
      .await
  }
  pub(crate) async fn reset(&self) -> Result<(), Error> {
    self
      .access(|d| {
        d.journal.done.fill(false);
        d.journal.etag = None;
        d.save()
      })
      .await
  }
  pub(crate) async fn write(
    &self,
    offset: u64,
    bytes: bytes::Bytes,
    index: Option<usize>,
    cancel: Cancel,
  ) -> Result<u64, Error> {
    self
      .access(move |d| {
        cancel.check()?;
        if offset > d.journal.length || bytes.len() as u64 > d.journal.length - offset {
          return Err(Error::Checkpoint("write out of range"));
        }
        if let Some(i) = index
          && (i >= d.journal.done.len()
            || offset != i as u64 * d.journal.chunk_size
            || bytes.len() as u64 != d.block_len(i))
        {
          return Err(Error::Checkpoint("invalid range commit"));
        }
        d.file.seek(SeekFrom::Start(offset))?;
        d.file.write_all(&bytes)?;
        if let Some(i) = index {
          d.file.sync_data()?;
          d.journal.done[i] = true;
          d.save()?;
        }
        Ok(d.staged())
      })
      .await
  }
  pub(crate) async fn full_complete(&self) -> Result<u64, Error> {
    self
      .access(|d| {
        d.file.sync_data()?;
        d.journal.done.fill(true);
        d.save()?;
        Ok(d.journal.length)
      })
      .await
  }
  pub(crate) async fn verify(
    &self,
    cancel: Cancel,
    progress: watch::Sender<Progress>,
  ) -> Result<(), Error> {
    self
      .access(move |d| {
        cancel.check()?;
        if !d.published && d.journal.done.iter().any(|done| !done) {
          return Err(Error::Checkpoint("file has missing ranges"));
        }
        if d.file.metadata()?.len() != d.journal.length {
          return Err(Error::Integrity);
        }
        d.file.seek(SeekFrom::Start(0))?;
        d.journal.checksum.verify(&mut d.file, |n| {
          cancel.check()?;
          progress.send_modify(|p| p.verification_bytes = n);
          Ok(())
        })?;
        if d.file.metadata()?.len() != d.journal.length {
          return Err(Error::Integrity);
        }
        Ok(())
      })
      .await
  }
  pub(crate) async fn publish(
    &self,
    cancel: Cancel,
    progress: watch::Sender<Progress>,
  ) -> Result<PathBuf, Error> {
    self
      .access(move |d| {
        cancel.check()?;
        if !d.published {
          d.file.sync_all()?;
          // Atomic no-clobber publication on the same filesystem. No check/rename
          // race; the lock file survives renaming and stays held until cleanup.
          progress.send_modify(|p| p.state = DownloadState::Publishing);
          cancel.check()?;
          fs::hard_link(&d.part, &d.target)?;
          d.published = true;
          sync_directory(d.target.parent().unwrap())?;
        }
        // A previous crash may have published the target before cleanup. Only
        // remove sidecars after the target has just passed its checksum again.
        for p in [&d.part, &d.journal_path] {
          match fs::remove_file(p) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
          }
        }
        sync_directory(d.target.parent().unwrap())?;
        Ok(d.target.clone())
      })
      .await
  }
}
