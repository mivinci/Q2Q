//! Root-bound, crash-recoverable block receipts for SDK downloads.

use crate::error::TransferError;
use q2q_primitives::block::PIECE_SIZE;
use q2q_primitives::file::{FileStore, StoreError};
use q2q_primitives::merkle::{Hash, Proof};
use q2q_primitives::peer_wire::MAX_CONTROL_BYTES;
use q2q_primitives::seed::Seed;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
#[cfg(test)]
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
  version: u32,
  info_hash: [u8; 32],
  #[serde(default)]
  target_hash: Option<[u8; 32]>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
  index: usize,
  proof: Proof,
}

pub(crate) struct DownloadStore {
  store: FileStore,
  // The OS releases this advisory lock on process exit. Blocking writes retain
  // the enclosing Arc, so cancellation cannot release it during a disk write.
  destination: File,
  journal: File,
  failed: bool,
  receipts: Vec<Option<u64>>,
}

#[cfg(test)]
impl Deref for DownloadStore {
  type Target = FileStore;
  fn deref(&self) -> &FileStore {
    &self.store
  }
}
#[cfg(test)]
impl DerefMut for DownloadStore {
  fn deref_mut(&mut self) -> &mut FileStore {
    &mut self.store
  }
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
  let mut name = path.as_os_str().to_os_string();
  name.push(suffix);
  name.into()
}

fn normalized_target_hash(path: &Path) -> Result<[u8; 32], TransferError> {
  let absolute = if path.is_absolute() {
    path.to_path_buf()
  } else {
    std::env::current_dir()?.join(path)
  };
  let file_name = absolute
    .file_name()
    .ok_or_else(|| invalid("download destination must name a file"))?;
  let parent = absolute.parent().unwrap_or(Path::new("."));
  let canonical_parent = std::fs::canonicalize(parent)?;
  let joined = canonical_parent.join(file_name);
  let normalized = match std::fs::canonicalize(&joined) {
    Ok(path) => path,
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => joined,
    Err(error) => return Err(error.into()),
  };
  Ok(Sha256::digest(normalized.as_os_str().as_encoded_bytes()).into())
}

fn central_journal_path(directory: &Path, target_hash: &[u8; 32]) -> PathBuf {
  let name: String = target_hash
    .iter()
    .map(|byte| format!("{byte:02x}"))
    .collect();
  directory.join(format!("{name}.q2q.resume"))
}

pub(crate) fn has_checkpoint(
  path: &Path,
  checkpoint_directory: &Path,
) -> Result<bool, TransferError> {
  let target_hash = normalized_target_hash(path)?;
  let journal_path = central_journal_path(checkpoint_directory, &target_hash);
  let legacy_path = sidecar(path, ".q2q.resume");
  Ok(journal_path.try_exists()? || legacy_path.try_exists()?)
}

fn clean_stale_temporary_files(journal_path: &Path) -> Result<(), TransferError> {
  let directory = journal_path
    .parent()
    .filter(|p| !p.as_os_str().is_empty())
    .unwrap_or(Path::new("."));
  let Some(name) = journal_path.file_name() else {
    return Ok(());
  };
  let mut prefix = name.to_os_string();
  prefix.push(".");
  for entry in std::fs::read_dir(directory)? {
    let entry = entry?;
    let file_name = entry.file_name();
    if !file_name
      .as_os_str()
      .as_encoded_bytes()
      .starts_with(prefix.as_os_str().as_encoded_bytes())
      || !file_name.to_string_lossy().ends_with(".tmp")
    {
      continue;
    }
    if entry.file_type()?.is_file() {
      std::fs::remove_file(entry.path())?;
    }
  }
  Ok(())
}

fn invalid(message: impl Into<String>) -> TransferError {
  TransferError::Checkpoint(message.into())
}

fn write_record(file: &mut File, value: &impl Serialize) -> Result<(), TransferError> {
  let mut bytes = serde_json::to_vec(value).map_err(|e| invalid(e.to_string()))?;
  if bytes.len() > MAX_CONTROL_BYTES {
    return Err(invalid("checkpoint record exceeds size limit"));
  }
  bytes.push(b'\n');
  file.write_all(&bytes)?;
  Ok(())
}

// Read one bounded record. A final torn append is discarded on resume.
fn read_record(reader: &mut impl BufRead) -> Result<Option<Vec<u8>>, TransferError> {
  let mut line = Vec::new();
  reader
    .take((MAX_CONTROL_BYTES + 2) as u64)
    .read_until(b'\n', &mut line)?;
  if line.len() > MAX_CONTROL_BYTES + 1 {
    return Err(invalid("checkpoint record exceeds size limit"));
  }
  if line.last() != Some(&b'\n') {
    return Ok(None);
  }
  line.pop();
  Ok(Some(line))
}

fn sync_parent(_path: &Path) -> Result<(), TransferError> {
  #[cfg(unix)]
  File::open(
    _path
      .parent()
      .filter(|p| !p.as_os_str().is_empty())
      .unwrap_or(Path::new(".")),
  )?
  .sync_all()?;
  Ok(())
}

struct Temporary(PathBuf);
impl Drop for Temporary {
  fn drop(&mut self) {
    let _ = std::fs::remove_file(&self.0);
  }
}

impl DownloadStore {
  pub(crate) fn verified_bitfield(&self) -> Vec<u8> {
    self.store.verified_bitfield()
  }
  pub(crate) fn complete(&self) -> Result<bool, TransferError> {
    for index in 0..self.store.block_count() {
      if self.store.state(index)? != q2q_primitives::block::BlockState::Verified {
        return Ok(false);
      }
    }
    Ok(true)
  }
  pub(crate) fn read_verified(&self, index: usize) -> Result<Vec<u8>, TransferError> {
    if self.store.state(index)? != q2q_primitives::block::BlockState::Verified {
      return Err(std::io::Error::other("block is not verified").into());
    }
    let bytes = self.store.read_block(index)?;
    if self.store.cache().leaf(index) != Some(&q2q_primitives::merkle::leaf_hash(&bytes)) {
      return Err(
        std::io::Error::new(
          std::io::ErrorKind::InvalidData,
          "verified destination block changed",
        )
        .into(),
      );
    }
    Ok(bytes)
  }
  pub(crate) fn upload_revision(
    &self,
    index: usize,
  ) -> Result<crate::upload_source::SourceRevision, TransferError> {
    if self.failed || self.receipts.get(index).copied().flatten().is_none() {
      return Err(invalid("block is unavailable for sharing"));
    }
    Ok(
      crate::upload_source::SourceRevision::from_metadata(&self.destination.metadata()?)
        .with_receipts(&self.journal.metadata()?),
    )
  }

  pub(crate) fn upload_block(
    &mut self,
    index: usize,
    seed: &Seed,
  ) -> Result<(Vec<u8>, Proof), TransferError> {
    if self.failed {
      return Err(invalid("destination is unavailable for sharing"));
    }
    let offset = self
      .receipts
      .get(index)
      .copied()
      .flatten()
      .ok_or_else(|| invalid("block has no committed receipt"))?;
    let bytes = self.read_verified(index)?;
    // All journal access is under Destination's lock. Appends seek to EOF;
    // a failed proof read cannot redirect a later checkpoint write.
    self.journal.seek(SeekFrom::Start(offset))?;
    let line = read_record(&mut BufReader::new(&mut self.journal))?
      .ok_or_else(|| invalid("missing committed receipt"))?;
    let receipt: Receipt = serde_json::from_slice(&line).map_err(|e| invalid(e.to_string()))?;
    if receipt.index != index
      || !q2q_primitives::merkle::verify(
        &bytes,
        index,
        seed.info.leaf_count(),
        &receipt.proof,
        &seed.info.root,
      )
    {
      return Err(invalid("shared block or proof changed"));
    }
    Ok((bytes, receipt.proof))
  }

  pub(crate) fn sync(&self) -> std::io::Result<()> {
    self.destination.sync_all()
  }

  pub(crate) fn create(seed: &Seed, path: &Path) -> Result<Self, TransferError> {
    Self::create_with_checkpoint(seed, path, None)
  }

  pub(crate) fn create_in(
    seed: &Seed,
    path: &Path,
    checkpoint_directory: &Path,
  ) -> Result<Self, TransferError> {
    Self::create_with_checkpoint(seed, path, Some(checkpoint_directory))
  }

  fn create_with_checkpoint(
    seed: &Seed,
    path: &Path,
    checkpoint_directory: Option<&Path>,
  ) -> Result<Self, TransferError> {
    let target_hash = checkpoint_directory
      .map(|_| normalized_target_hash(path))
      .transpose()?;
    if let Some(directory) = checkpoint_directory {
      std::fs::create_dir_all(directory)?;
    }
    let journal_path = match (checkpoint_directory, target_hash.as_ref()) {
      (Some(directory), Some(hash)) => central_journal_path(directory, hash),
      _ => sidecar(path, ".q2q.resume"),
    };
    let legacy_path = sidecar(path, ".q2q.resume");
    let destination_exists = path.try_exists()?;
    if checkpoint_directory.is_some() && !destination_exists {
      // The target was deleted, so its old receipts cannot help recovery.
      for stale_path in [&journal_path, &legacy_path] {
        if stale_path.try_exists()? {
          std::fs::remove_file(stale_path)?;
          sync_parent(stale_path)?;
        }
      }
    }
    if journal_path.try_exists()? || (checkpoint_directory.is_some() && legacy_path.try_exists()?) {
      return Err(
        std::io::Error::new(
          std::io::ErrorKind::AlreadyExists,
          "download checkpoint already exists",
        )
        .into(),
      );
    }
    let destination = OpenOptions::new()
      .read(true)
      .write(true)
      .create_new(true)
      .open(path)?;
    destination
      .try_lock()
      .map_err(|e| invalid(format!("destination is locked: {e}")))?;
    clean_stale_temporary_files(&journal_path)?;
    // A stale receipt file must not be silently attached to a new destination.
    let mut journal = OpenOptions::new()
      .read(true)
      .write(true)
      .create_new(true)
      .open(&journal_path)?;
    destination.set_len(seed.info.length)?;
    destination.sync_all()?;
    write_record(
      &mut journal,
      &Header {
        version: if checkpoint_directory.is_some() { 2 } else { 1 },
        info_hash: seed.info_hash(),
        target_hash,
      },
    )?;
    journal.sync_all()?;
    sync_parent(path)?;
    if checkpoint_directory.is_some() {
      sync_parent(&journal_path)?;
    }
    Ok(Self {
      store: FileStore::open(path, seed.info.length, seed.info.block_size)?,
      destination,
      journal,
      failed: false,
      receipts: vec![None; seed.info.leaf_count()],
    })
  }

  pub(crate) fn resume(seed: &Seed, path: &Path) -> Result<Self, TransferError> {
    Self::resume_with_checkpoint(seed, path, None)
  }

  pub(crate) fn resume_in(
    seed: &Seed,
    path: &Path,
    checkpoint_directory: &Path,
  ) -> Result<Self, TransferError> {
    Self::resume_with_checkpoint(seed, path, Some(checkpoint_directory))
  }

  fn resume_with_checkpoint(
    seed: &Seed,
    path: &Path,
    checkpoint_directory: Option<&Path>,
  ) -> Result<Self, TransferError> {
    let target_hash = checkpoint_directory
      .map(|_| normalized_target_hash(path))
      .transpose()?;
    if let Some(directory) = checkpoint_directory {
      std::fs::create_dir_all(directory)?;
    }
    let journal_path = match (checkpoint_directory, target_hash.as_ref()) {
      (Some(directory), Some(hash)) => central_journal_path(directory, hash),
      _ => sidecar(path, ".q2q.resume"),
    };
    let legacy_path = sidecar(path, ".q2q.resume");
    let destination = OpenOptions::new().read(true).write(true).open(path)?;
    destination
      .try_lock()
      .map_err(|e| invalid(format!("destination is locked: {e}")))?;
    let mut store = FileStore::open(path, seed.info.length, seed.info.block_size)?;
    let legacy_exists = checkpoint_directory.is_some() && legacy_path.try_exists()?;
    let central_exists = checkpoint_directory.is_some() && journal_path.try_exists()?;
    let source_journal_path = if legacy_exists && !central_exists {
      &legacy_path
    } else {
      &journal_path
    };
    let is_legacy = checkpoint_directory.is_some() && source_journal_path == &legacy_path;
    clean_stale_temporary_files(&journal_path)?;
    if checkpoint_directory.is_some() {
      clean_stale_temporary_files(&legacy_path)?;
    }
    let mut reader = BufReader::new(File::open(source_journal_path)?);
    let header: Header = serde_json::from_slice(
      &read_record(&mut reader)?.ok_or_else(|| invalid("missing checkpoint header"))?,
    )
    .map_err(|e| invalid(e.to_string()))?;
    let valid_header = if let Some(hash) = target_hash {
      if is_legacy {
        header.version == 1 && header.target_hash.is_none()
      } else {
        header.version == 2 && header.target_hash == Some(hash)
      }
    } else {
      header.version == 1 && header.target_hash.is_none()
    };
    if !valid_header || header.info_hash != seed.info_hash() {
      return Err(invalid("checkpoint belongs to another seed or version"));
    }
    // A unique temporary file avoids following or truncating an existing path.
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce).map_err(|e| invalid(e.to_string()))?;
    let nonce: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
    let temporary_path = sidecar(&journal_path, &format!(".{nonce}.tmp"));
    let file = OpenOptions::new()
      .read(true)
      .write(true)
      .create_new(true)
      .open(&temporary_path)?;
    let temporary = Temporary(temporary_path);
    let mut journal = file;
    let compacted_header = if let Some(hash) = target_hash {
      Header {
        version: 2,
        info_hash: header.info_hash,
        target_hash: Some(hash),
      }
    } else {
      Header {
        version: header.version,
        info_hash: header.info_hash,
        target_hash: header.target_hash,
      }
    };
    write_record(&mut journal, &compacted_header)?;
    let mut seen = vec![false; store.block_count()];
    let mut receipts = vec![None; store.block_count()];
    while let Some(line) = read_record(&mut reader)? {
      let receipt: Receipt = serde_json::from_slice(&line).map_err(|e| invalid(e.to_string()))?;
      if receipt.index >= seen.len() || seen[receipt.index] {
        return Err(invalid("checkpoint block index is invalid or duplicated"));
      }
      seen[receipt.index] = true;
      match store.verify_existing(receipt.index, &receipt.proof, &seed.info.root) {
        Ok(_) => {
          let offset = journal.stream_position()?;
          write_record(&mut journal, &receipt)?;
          receipts[receipt.index] = Some(offset);
        }
        // Corrupt bytes or forged receipts are missing blocks, never verified.
        Err(StoreError::RootMismatch) => {}
        Err(error) => return Err(error.into()),
      }
    }
    drop(reader);
    // Compact validated receipts before appending. Rename commits the complete
    // replacement atomically; the old journal survives interruption beforehand.
    journal.sync_all()?;
    drop(journal);
    std::fs::rename(&temporary.0, &journal_path)?;
    sync_parent(&journal_path)?;
    if legacy_exists {
      std::fs::remove_file(&legacy_path)?;
      sync_parent(&legacy_path)?;
    }
    let mut journal = OpenOptions::new()
      .read(true)
      .append(true)
      .open(journal_path)?;
    journal.seek(SeekFrom::End(0))?;
    Ok(Self {
      store,
      destination,
      journal,
      failed: false,
      receipts,
    })
  }

  pub(crate) fn commit(
    &mut self,
    index: usize,
    bytes: &[u8],
    proof: Proof,
    root: &Hash,
  ) -> Result<(), TransferError> {
    if self.failed {
      return Err(invalid(
        "destination stopped after a previous disk/checkpoint failure",
      ));
    }
    let result = self.commit_block(index, bytes, proof, root);
    self.failed = result.is_err();
    result
  }

  fn commit_block(
    &mut self,
    index: usize,
    bytes: &[u8],
    proof: Proof,
    root: &Hash,
  ) -> Result<(), TransferError> {
    for (piece, data) in bytes.chunks(PIECE_SIZE).enumerate() {
      self.store.write_piece(index, piece, data)?;
    }
    self
      .store
      .verify(index, Some(&proof), root)
      .map_err(|error| match error {
        StoreError::RootMismatch => invalid("written block does not match its verified proof"),
        other => TransferError::Store(other),
      })?;
    // Durable bytes precede their receipt. An interrupted write has no receipt
    // and will simply be requested again after restart.
    self.destination.sync_data()?;
    let offset = self.journal.seek(SeekFrom::End(0))?;
    write_record(&mut self.journal, &Receipt { index, proof })?;
    self.journal.sync_data()?;
    // Publish the index only after both data and receipt are durable.
    self.receipts[index] = Some(offset);
    Ok(())
  }
}

#[cfg(test)]
#[path = "../tests/download_store.rs"]
pub(crate) mod tests;
