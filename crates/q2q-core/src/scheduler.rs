//! Bounded peer workers and interruption at source-policy deadlines.
pub(crate) use crate::block_queue::WorkQueue;
use crate::destination::Destination;
use crate::error::TransferError;
use crate::peer::PeerSession;
pub(crate) use crate::task_scope::TaskScope;
use q2q_primitives::seed::Seed;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

pub(crate) async fn guard_peer<F: Future<Output = Result<T, TransferError>>, T>(
  queue: &WorkQueue,
  index: usize,
  operation: F,
) -> Result<T, TransferError> {
  tokio::pin!(operation);
  let mut changed = queue.subscribe();
  loop {
    if let Some(deadline) = queue.peer_deadline(index) {
      tokio::select! { result = &mut operation => return result, _ = tokio::time::sleep_until(deadline) => return Err(TransferError::Timeout), _ = changed.changed() => {} }
    } else {
      tokio::select! { result = &mut operation => return result, _ = changed.changed() => {} }
    }
  }
}

pub(crate) async fn download_peer(
  peer: PeerSession,
  seed: &Seed,
  store: Destination,
  queue: Arc<WorkQueue>,
) -> Result<(), TransferError> {
  let observation = queue.peers.connection(peer.peer_id().clone());
  download_peer_observed(peer, seed, store, queue, observation).await
}

#[tracing::instrument(skip_all, fields(connection_id = %peer.connection_id(), peer_id = %peer.peer_id().as_str()))]
pub(crate) async fn download_peer_observed(
  peer: PeerSession,
  seed: &Seed,
  store: Destination,
  queue: Arc<WorkQueue>,
  observation: Arc<crate::download_progress::Connection>,
) -> Result<(), TransferError> {
  use crate::download_progress::DownloadPeerState as State;
  observation.connected(peer.connection_id(), peer.peer_id());
  let mut connection = peer.begin_download(seed, queue.bitfield()).await?;
  connection.observe(observation.clone());
  observation.state(State::Idle);
  let _active = crate::progress::ActivePeer::new(queue.progress.clone());
  let mut changed = queue.subscribe();
  let mut idle = tokio::time::Instant::now() + Duration::from_secs(3);
  loop {
    changed.borrow_and_update();
    if queue.complete() {
      break;
    }
    if let Some(mut lease) = queue.claim_available(&connection.available()) {
      let index = lease.index;
      let length =
        crate::progress::block_len(seed.info.length, seed.info.block_size, index) as usize;
      observation.state(State::Receiving);
      let (bytes, proof) = guard_peer(&queue, index, connection.fetch(index, length))
        .await
        .inspect_err(|error| {
          tracing::debug!(block_index = index, %error, stage = "block_fetch", "Block fetch failed");
        })?;
      observation.state(State::Verifying);
      lease.attribute(&observation);
      store.commit(lease, bytes, proof).await.inspect_err(|error| {
        tracing::debug!(block_index = index, %error, stage = "block_commit", "Block verification or persistence failed");
      })?;
      tracing::debug!(block_index = index, "Block verified and committed");
      connection.acknowledge(index).await?;
      observation.state(State::Idle);
      idle = tokio::time::Instant::now() + Duration::from_secs(3);
    } else {
      // Unrelated Have messages cannot keep a useless peer occupying a slot.
      tokio::select! {
        result = connection.receive_have() => result?,
        _ = changed.changed() => {},
        _ = tokio::time::sleep_until(idle) => break,
      }
    }
  }
  observation.state(State::Finishing);
  tracing::debug!(stage = "finish_ack", "Finishing download peer session");
  connection.finish().await.inspect_err(|error| {
    tracing::debug!(%error, stage = "finish_ack", "Download session completion failed");
  })
}

#[cfg(test)]
#[path = "../tests/scheduler.rs"]
mod tests;

#[cfg(test)]
pub(crate) use tests::download;
