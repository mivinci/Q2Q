//! Private fixtures for fault injection and low-level transfer tests.
use crate::destination::Destination;
use crate::error::TransferError;
use q2q_core::seed::Seed;
use std::path::Path;

pub(crate) async fn create_destination(
  seed: &Seed,
  path: &Path,
) -> Result<Destination, TransferError> {
  open_destination(seed, path, false).await
}

pub(crate) async fn open_destination(
  seed: &Seed,
  path: &Path,
  resume: bool,
) -> Result<Destination, TransferError> {
  Destination::open(seed, path, resume).await
}

pub(crate) fn destination_complete(store: &Destination) -> Result<bool, TransferError> {
  let store = store
    .lock()
    .map_err(|_| TransferError::Checkpoint("destination lock poisoned".into()))?;
  for index in 0..store.block_count() {
    if store.state(index)? != q2q_core::block::BlockState::Verified {
      return Ok(false);
    }
  }
  Ok(true)
}

pub(crate) async fn sync_destination(path: &Path) -> Result<(), TransferError> {
  let path = path.to_path_buf();
  tokio::task::spawn_blocking(move || {
    std::fs::OpenOptions::new()
      .write(true)
      .open(path)?
      .sync_all()
  })
  .await??;
  Ok(())
}
