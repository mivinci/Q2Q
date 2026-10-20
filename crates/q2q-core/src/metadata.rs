//! Bounded, content-addressed metadata discovery; no destination is opened here.
use crate::{
  client::Client,
  control::{ControlError, QueryOptions},
  error::TransferError,
};
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use q2q_primitives::{
  peer_wire::Control,
  seed::{Info, MAX_INFO_BYTES, Magnet, Seed, SeedError},
};
use std::{
  collections::HashSet,
  io::{self, Read, Write},
  path::{Path, PathBuf},
  time::Duration,
};
use tokio::task::JoinSet;
use webrtc::data_channel::DataChannelEvent;

#[derive(Debug)]
pub enum MetadataError {
  Control(ControlError),
  Peer(TransferError),
  Invalid(SeedError),
  Cache(io::Error),
  Task(tokio::task::JoinError),
  NoPeers,
  Timeout,
  Unsupported,
  Exhausted {
    tried: usize,
    last: Box<MetadataError>,
  },
}
impl std::fmt::Display for MetadataError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      Self::Control(_) => f.write_str("metadata Track session or discovery failed"),
      Self::Peer(e) => write!(f, "metadata Peer: {e}"),
      Self::Invalid(e) => write!(f, "invalid metadata: {e}"),
      Self::Cache(e) => write!(f, "metadata cache: {e}"),
      Self::Task(e) => write!(f, "metadata task: {e}"),
      Self::NoPeers => f.write_str("no Peer available to provide metadata"),
      Self::Timeout => f.write_str("metadata discovery timed out"),
      Self::Unsupported => f.write_str("Peer did not support the metadata exchange"),
      Self::Exhausted { tried, last } => {
        write!(f, "metadata candidates exhausted ({tried} tried): {last}")
      }
    }
  }
}
impl std::error::Error for MetadataError {
  fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
    match self {
      Self::Control(e) => Some(e),
      Self::Peer(e) => Some(e),
      Self::Invalid(e) => Some(e),
      Self::Cache(e) => Some(e),
      Self::Task(e) => Some(e),
      Self::Exhausted { last, .. } => Some(last),
      _ => None,
    }
  }
}

fn cache_path(root: &Path, magnet: &Magnet) -> PathBuf {
  root
    .join("metadata")
    .join(format!("{}.q2q", crate::control::hex(&magnet.info_hash)))
}

/// Read verified metadata without creating an identity or contacting Track.
/// Corrupt/foreign cache entries are misses; filesystem failures are reported.
pub async fn cached_magnet(
  magnet: &Magnet,
  storage_dir: impl AsRef<Path>,
) -> Result<Option<Seed>, MetadataError> {
  let path = cache_path(storage_dir.as_ref(), magnet);
  let hash = magnet.info_hash;
  tokio::task::spawn_blocking(move || {
    let file = match std::fs::File::open(path) {
      Ok(file) => file,
      Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
      Err(e) => return Err(MetadataError::Cache(e)),
    };
    let mut bytes = Vec::new();
    file
      .take((MAX_INFO_BYTES + 64) as u64)
      .read_to_end(&mut bytes)
      .map_err(MetadataError::Cache)?;
    if bytes.len() > MAX_INFO_BYTES + 32 {
      return Ok(None);
    }
    let Ok(seed) = Seed::parse(&bytes) else {
      return Ok(None);
    };
    if seed.info_hash() != hash
      || seed.info.encode().len() > MAX_INFO_BYTES
      || crate::destination::check_seed(&seed).is_err()
    {
      return Ok(None);
    }
    // Cached hints are never used to introduce new remote URLs.
    Ok(Some(Seed::new(seed.info)))
  })
  .await
  .map_err(MetadataError::Task)?
}

async fn store_cache(root: &Path, magnet: &Magnet, seed: &Seed) -> Result<(), MetadataError> {
  let path = cache_path(root, magnet);
  let bytes = Seed::new(seed.info.clone()).encode();
  tokio::task::spawn_blocking(move || -> io::Result<()> {
    let parent = path.parent().expect("metadata parent");
    std::fs::create_dir_all(parent)?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    staged.write_all(&bytes)?;
    staged.as_file().sync_all()?;
    staged.persist(path).map_err(|e| e.error)?;
    Ok(())
  })
  .await
  .map_err(MetadataError::Task)?
  .map_err(MetadataError::Cache)
}

pub(crate) async fn resolve(
  client: &Client,
  magnet: &Magnet,
  concurrency: usize,
  query_limit: usize,
) -> Result<Seed, MetadataError> {
  if let Some(seed) = cached_magnet(magnet, client.storage_dir()).await? {
    return Ok(seed);
  }
  let seed = tokio::time::timeout(Duration::from_secs(60), async {
    let session = client.connect().await.map_err(MetadataError::Control)?;
    let peers = session
      .query_with_options(
        &magnet.info_hash,
        QueryOptions {
          limit: Some(query_limit),
          cursor: None,
        },
      )
      .await
      .map_err(MetadataError::Control)?
      .peers;
    let mut seen = HashSet::new();
    let mut candidates = peers
      .into_iter()
      .filter(|peer| peer.peer_id != client.peer_id().as_str() && seen.insert(peer.peer_id.clone()))
      .take(query_limit);
    let mut tasks = JoinSet::new();
    let mut last = MetadataError::NoPeers;
    let mut tried = 0;
    let fetching = async {
      loop {
        while tasks.len() < concurrency {
          let Some(target) = candidates.next() else {
            break;
          };
          let session = session.clone();
          let hash = magnet.info_hash;
          tried += 1;
          tasks.spawn(async move {
            tokio::time::timeout(Duration::from_secs(45), async {
              let peer = session
                .dial_peer(&target, hash)
                .await
                .map_err(MetadataError::Control)?;
              let result = peer.request_info().await;
              let closed = peer.close().await;
              match result {
                Ok(seed) => {
                  closed.map_err(MetadataError::Control)?;
                  Ok(seed)
                }
                Err(e) => Err(e),
              }
            })
            .await
            .map_err(|_| MetadataError::Timeout)?
          });
        }
        let Some(result) = tasks.join_next().await else {
          return Err(if tried == 0 {
            MetadataError::NoPeers
          } else {
            MetadataError::Exhausted {
              tried,
              last: Box::new(last),
            }
          });
        };
        match result.map_err(MetadataError::Task)? {
          Ok(seed) => {
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
            return Ok(seed);
          }
          Err(error) => last = error,
        }
      }
    };
    tokio::select! {
      result = fetching => result,
      error = session.keep_online() => Err(MetadataError::Control(error)),
    }
  })
  .await
  .map_err(|_| MetadataError::Timeout)??;
  store_cache(client.storage_dir(), magnet, &seed).await?;
  Ok(seed)
}

impl crate::peer::PeerSession {
  async fn request_info(&self) -> Result<Seed, MetadataError> {
    let exchange = async {
      crate::peer_transfer::send(&*self.control, Control::RequestInfo {})
        .await
        .map_err(MetadataError::Peer)?;
      let response = crate::peer_transfer::receive(&*self.control)
        .await
        .map_err(MetadataError::Peer)?;
      let Control::Info { data } = response else {
        return Err(MetadataError::Unsupported);
      };
      if data.len() > MAX_INFO_BYTES.div_ceil(3) * 4 {
        return Err(MetadataError::Invalid(SeedError::InfoTooLarge));
      }
      let bytes = B64.decode(data).map_err(|_| {
        MetadataError::Peer(crate::peer_transfer::protocol("metadata is not base64"))
      })?;
      let info = Info::parse_verified(&bytes, self.info_hash()).map_err(MetadataError::Invalid)?;
      let seed = Seed::new(info);
      crate::destination::check_seed(&seed).map_err(MetadataError::Peer)?;
      Ok(seed)
    };
    let data_events = async {
      loop {
        match self.data.poll().await {
          Some(DataChannelEvent::OnMessage(_)) => return Err(MetadataError::Unsupported),
          Some(DataChannelEvent::OnClose) | None => {
            return Err(MetadataError::Peer(TransferError::Closed));
          }
          _ => {}
        }
      }
    };
    tokio::time::timeout(Duration::from_secs(10), async {
      tokio::select! { result = exchange => result, result = data_events => result }
    })
    .await
    .map_err(|_| MetadataError::Timeout)?
  }
}

#[cfg(test)]
#[path = "../tests/metadata.rs"]
mod tests;
