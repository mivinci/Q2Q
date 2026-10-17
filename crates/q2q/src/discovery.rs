//! Bounded candidate refresh and peer retry policy shared by downloads and VOD.
use crate::{
  block_queue::WorkQueue, control::ControlSession, destination::Destination, error::TransferError,
};
use q2q_core::seed::Seed;
use q2q_track::wire::PeerWire;
use std::{
  collections::{HashMap, HashSet},
  sync::Arc,
  time::Duration,
};
use tokio::{task::JoinSet, time::Instant};

pub(crate) struct Peers {
  pub(crate) session: Arc<ControlSession>,
  pub(crate) candidates: Vec<PeerWire>,
  pub(crate) concurrency: usize,
}
impl Peers {
  pub(crate) async fn run(
    self,
    seed: Arc<Seed>,
    store: Destination,
    queue: Arc<WorkQueue>,
    continuous: bool,
  ) -> Result<(), TransferError> {
    let mut candidates = self.candidates;
    let mut active = HashSet::new();
    let mut retry = HashMap::<String, Instant>::new();
    let mut workers: JoinSet<(String, Result<(), TransferError>)> = JoinSet::new();
    let mut changes = queue.subscribe();
    let mut focus = queue.subscribe_focus();
    let mut refresh = Instant::now() + Duration::from_secs(5);
    let mut stalled = Instant::now();
    let mut verified = queue.progress.borrow().verified_blocks;
    let mut last = TransferError::NoPeers;
    let mut exhausted = None;
    loop {
      if queue.complete() {
        // Every BlockDone/end handshake must finish before dropping workers.
        // Verified data alone is not permission to abort the final ACK.
        while let Some(result) = workers.join_next().await {
          let (_, result) = result?;
          if let Err(error) = result
            && !error.peer_retryable()
          {
            return Err(error);
          }
        }
        return Ok(());
      }
      let now_verified = queue.progress.borrow().verified_blocks;
      if now_verified != verified {
        verified = now_verified;
        stalled = Instant::now();
      }
      // Only the current bounded Track page is retained. Complete peers go
      // first, while partial peers can fill spare slots and receive Have.
      candidates.sort_by_key(|peer| !peer.complete);
      let mut seen = HashSet::new();
      candidates.retain(|peer| {
        peer.peer_id != self.session.peer_id().as_str() && seen.insert(peer.peer_id.clone())
      });
      candidates.truncate(32);
      for target in &candidates {
        if active.len() >= self.concurrency {
          break;
        }
        if active.contains(&target.peer_id)
          || retry
            .get(&target.peer_id)
            .is_some_and(|until| *until > Instant::now())
        {
          continue;
        }
        let target = target.clone();
        let id = target.peer_id.clone();
        active.insert(id.clone());
        let session = self.session.clone();
        let seed = seed.clone();
        let store = store.clone();
        let queue = queue.clone();
        let task = queue.scope.enter();
        workers.spawn(async move {
          let _task = task;
          let result = async {
            let peer = tokio::time::timeout(
              Duration::from_secs(45),
              session.dial_peer(&target, seed.info_hash()),
            )
            .await
            .map_err(|_| TransferError::Timeout)??;
            crate::scheduler::download_peer(peer, &seed, store, queue).await
          }
          .await;
          (id, result)
        });
      }
      if exhausted != Some(active.is_empty()) {
        exhausted = Some(active.is_empty());
        queue.peer_status(active.is_empty(), None);
      }
      if !continuous && active.is_empty() && stalled.elapsed() >= Duration::from_secs(10) {
        return Err(last);
      }
      let wake = retry
        .values()
        .filter(|until| **until > Instant::now())
        .copied()
        .min()
        .map_or(refresh, |until| until.min(refresh));
      tokio::select! {
        _ = queue.finished() => {
          if !queue.complete() { return Err(TransferError::Protocol("download queue stopped".into())); }
        },
        _ = changes.changed() => {},
        _ = focus.changed() => { retry.clear(); refresh = Instant::now(); },
        result = workers.join_next(), if !workers.is_empty() => {
          let (id, result) = result.expect("worker exists")?;
          active.remove(&id);
          retry.insert(id, Instant::now() + Duration::from_secs(5));
          match result {
            Ok(()) => {},
            Err(error) if error.peer_retryable() || matches!(error, TransferError::NoPeers) => last = error,
            Err(error) => return Err(error),
          }
        }
        _ = tokio::time::sleep_until(wake) => {
          if Instant::now() >= refresh {
            // Query errors do not cancel healthy connections. Keep existing
            // candidates and apply the same bounded retry schedule.
            if let Ok(response) = self.session.query(&seed.info_hash()).await { candidates = response.peers; }
            retry.retain(|id, _| active.contains(id) || candidates.iter().any(|peer| peer.peer_id == *id));
            refresh = Instant::now() + Duration::from_secs(5);
          }
        }
      }
    }
  }
}
