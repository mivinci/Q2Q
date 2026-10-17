//! Client construction and identity lifecycle.

pub use crate::control::{ControlError, ControlSession};
pub use crate::peer::PeerSession;
pub use crate::publication::Publication;
pub use crate::transfer::{Transfer, TransferError, TransferProgress, TransferState};

use crate::identity::Identity;
use crate::publication::{PrepareError, PreparedFile};
use q2q_core::identity::PeerId;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::AtomicBool};
use tokio::sync::OnceCell;
use url::Url;

/// Start configuring a client. The builder performs no network requests.
pub fn builder() -> Builder {
  Builder::default()
}

#[derive(Default)]
pub struct Builder {
  track_url: Option<String>,
  identity_file: Option<PathBuf>,
  storage_dir: Option<PathBuf>,
  max_download_peers: Option<usize>,
}

impl Builder {
  /// Track's logical HTTP(S) address. Punch and TURN endpoints arrive later
  /// from Track's announce response.
  pub fn track_url(mut self, url: impl Into<String>) -> Self {
    self.track_url = Some(url.into());
    self
  }

  /// A versioned file containing the local Ed25519 private seed. A missing
  /// file is created with owner-only permissions on Unix; an invalid existing
  /// file is rejected instead of silently changing this peer's identity.
  pub fn identity_file(mut self, path: impl Into<PathBuf>) -> Self {
    self.identity_file = Some(path.into());
    self
  }

  /// Directory reserved for verified blocks and download progress.
  pub fn storage_dir(mut self, path: impl Into<PathBuf>) -> Self {
    self.storage_dir = Some(path.into());
    self
  }

  /// Maximum concurrent peer connections or CDN block requests, 1–16 (default 4).
  pub fn max_download_peers(mut self, count: usize) -> Self {
    self.max_download_peers = Some(count);
    self
  }

  /// Validate the configuration and load or create the persistent identity.
  /// No connection to Track or Punch is opened yet.
  pub async fn build(self) -> Result<Client, BuildError> {
    let max_download_peers = self.max_download_peers.unwrap_or(4);
    if !(1..=16).contains(&max_download_peers) {
      return Err(BuildError::InvalidDownloadPeers);
    }
    let track_url = self.track_url.ok_or(BuildError::Missing("track_url"))?;
    let track_url = Url::parse(&track_url).map_err(BuildError::InvalidTrackUrl)?;
    if !matches!(track_url.scheme(), "http" | "https") || track_url.host_str().is_none() {
      return Err(BuildError::UnsupportedTrackUrl);
    }
    let identity_file = self
      .identity_file
      .ok_or(BuildError::Missing("identity_file"))?;
    let storage_dir = self.storage_dir.ok_or(BuildError::Missing("storage_dir"))?;

    tokio::fs::create_dir_all(&storage_dir)
      .await
      .map_err(BuildError::Storage)?;
    let identity = tokio::task::spawn_blocking(move || Identity::load_or_create(&identity_file))
      .await
      .map_err(BuildError::IdentityTask)?
      .map_err(BuildError::Identity)?;

    Ok(Client {
      track_url,
      storage_dir,
      identity,
      session: OnceCell::new(),
      publishing: Arc::new(AtomicBool::new(false)),
      max_download_peers,
    })
  }
}

/// A local peer that can publish and download through a shared control session.
pub struct Client {
  track_url: Url,
  storage_dir: PathBuf,
  identity: Identity,
  session: OnceCell<Arc<ControlSession>>,
  publishing: Arc<AtomicBool>,
  max_download_peers: usize,
}

impl Client {
  /// Reuse or establish the authenticated Track session and Punch Binding.
  /// Publication/download handles maintain heartbeats while active; callers
  /// using this lower-level session alone must drive its heartbeat themselves.
  pub async fn connect(&self) -> Result<Arc<ControlSession>, ControlError> {
    self
      .session
      .get_or_try_init(|| async {
        ControlSession::connect(&self.identity, &self.track_url)
          .await
          .map(Arc::new)
      })
      .await
      .cloned()
  }

  /// Publish one local file and maintain its announcement until the handle is
  /// closed or dropped. This first transfer stage permits one active source
  /// per client; the same client can download while publishing.
  pub async fn publish_file(&self, path: impl Into<PathBuf>) -> Result<Publication, TransferError> {
    let active = crate::publication::ActivePublication::acquire(self.publishing.clone())?;
    let prepared = self.prepare_file(path).await?;
    crate::transfer::check_seed(prepared.seed())?;
    let session = self.connect().await?;
    Publication::start(prepared, session, active).await
  }

  /// Download from complete peers concurrently, reassigning failed blocks.
  /// `wait` succeeds only after every block has
  /// passed verification; existing destination files are never overwritten.
  pub async fn download(
    &self,
    seed: &q2q_core::seed::Seed,
    path: impl Into<PathBuf>,
  ) -> Result<Transfer, TransferError> {
    self.start_download(seed, path.into(), false).await
  }

  /// Resume an existing SDK destination using its `.q2q.resume` receipts.
  /// Every recorded block is read and reverified against the seed root; missing
  /// or corrupt blocks are fetched again. A complete file needs no network.
  pub async fn resume_download(
    &self,
    seed: &q2q_core::seed::Seed,
    path: impl Into<PathBuf>,
  ) -> Result<Transfer, TransferError> {
    self.start_download(seed, path.into(), true).await
  }

  async fn start_download(
    &self,
    seed: &q2q_core::seed::Seed,
    path: PathBuf,
    resume: bool,
  ) -> Result<Transfer, TransferError> {
    crate::transfer::check_seed(seed)?;
    let initial_store = if resume {
      Some(crate::transfer::open_destination(seed, &path, true).await?)
    } else {
      None
    };
    let bits = match &initial_store {
      Some(store) => store
        .lock()
        .map_err(|_| TransferError::Protocol("destination lock poisoned".into()))?
        .verified_bitfield(),
      None => Vec::new(),
    };
    let progress = crate::progress::channel(seed, &bits);
    if let Some(store) = &initial_store
      && crate::transfer::destination_complete(store)?
    {
      return Ok(Transfer::start(progress, async move {
        let _store = initial_store;
        crate::transfer::sync_destination(&path).await?;
        Ok(path)
      }));
    }
    if !resume && tokio::fs::try_exists(&path).await? {
      return Err(
        std::io::Error::new(
          std::io::ErrorKind::AlreadyExists,
          "download destination already exists",
        )
        .into(),
      );
    }
    if seed.info.length == 0 {
      let seed = seed.clone();
      return Ok(Transfer::start(progress, async move {
        let _store = crate::transfer::create_destination(&seed, &path).await?;
        crate::transfer::sync_destination(&path).await?;
        Ok(path)
      }));
    }
    // A failed control plane must not prevent CDN-only cold start. Bound the
    // complete discovery path, including Punch Binding setup, before fallback.
    let discovery = tokio::time::timeout(std::time::Duration::from_secs(15), async {
      let session = self.connect().await?;
      let candidates = session.query(&seed.info_hash()).await?;
      let mut seen = std::collections::HashSet::new();
      let targets: Vec<_> = candidates
        .peers
        .into_iter()
        .filter(|peer| {
          peer.complete
            && peer.peer_id != self.peer_id().as_str()
            && seen.insert(peer.peer_id.clone())
        })
        .collect();
      if targets.is_empty() {
        return Err(TransferError::NoPeers);
      }
      Ok((session, targets))
    })
    .await
    .map_err(|_| TransferError::Timeout)
    .and_then(|result| result);
    let discovery = match discovery {
      Err(error) if seed.cdn_list.is_empty() => return Err(error),
      result => result,
    };
    let seed = seed.clone();
    let concurrency = self.max_download_peers;
    let updates = progress.clone();
    Ok(Transfer::start(progress, async move {
      let store = match initial_store {
        Some(store) => store,
        None => crate::transfer::create_destination(&seed, &path).await?,
      };
      let activity = crate::scheduler::TaskScope::new();
      let p2p = match discovery {
        Ok((session, targets)) => {
          let dialing = session.clone();
          let info_hash = seed.info_hash();
          let attempt = crate::scheduler::download_scoped(
            targets,
            seed.clone(),
            store.clone(),
            concurrency,
            updates.clone(),
            move |target| {
              let session = dialing.clone();
              async move {
                session
                  .dial_peer(&target, info_hash)
                  .await
                  .map_err(Into::into)
              }
            },
            activity.clone(),
          );
          tokio::select! {
            result = attempt => result,
            result = crate::publication::keep_online(&session) => Err(TransferError::Control(result)),
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
        crate::cdn::download(seed, store.clone(), concurrency, updates).await?;
      }
      crate::transfer::sync_destination(&path).await?;
      Ok(path)
    }))
  }

  /// Prepare a local file for seeding without announcing it to the network.
  /// The returned handle checks each block again before it can be uploaded.
  pub async fn prepare_file(&self, path: impl Into<PathBuf>) -> Result<PreparedFile, PrepareError> {
    let path = path.into();
    let track_url = self.track_url.to_string();
    tokio::task::spawn_blocking(move || PreparedFile::open(path, track_url))
      .await
      .map_err(PrepareError::Task)?
  }

  pub fn peer_id(&self) -> &PeerId {
    self.identity.peer_id()
  }

  pub fn public_key(&self) -> [u8; 32] {
    self.identity.public_key()
  }

  pub fn track_url(&self) -> &Url {
    &self.track_url
  }

  pub fn storage_dir(&self) -> &Path {
    &self.storage_dir
  }
}

#[derive(Debug)]
pub enum BuildError {
  Missing(&'static str),
  InvalidTrackUrl(url::ParseError),
  UnsupportedTrackUrl,
  InvalidDownloadPeers,
  Storage(io::Error),
  Identity(crate::identity::IdentityError),
  IdentityTask(tokio::task::JoinError),
}

impl fmt::Display for BuildError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::Missing(field) => write!(f, "missing client setting: {field}"),
      Self::InvalidTrackUrl(e) => write!(f, "invalid Track URL: {e}"),
      Self::InvalidDownloadPeers => f.write_str("max_download_peers must be between 1 and 16"),
      Self::UnsupportedTrackUrl => f.write_str("Track URL must be HTTP or HTTPS with a host"),
      Self::Storage(e) => write!(f, "could not prepare client storage: {e}"),
      Self::Identity(e) => write!(f, "could not load client identity: {e}"),
      Self::IdentityTask(e) => write!(f, "identity task failed: {e}"),
    }
  }
}

impl std::error::Error for BuildError {
  fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
    match self {
      Self::InvalidTrackUrl(e) => Some(e),
      Self::Storage(e) => Some(e),
      Self::Identity(e) => Some(e),
      Self::IdentityTask(e) => Some(e),
      _ => None,
    }
  }
}

#[cfg(test)]
#[path = "../tests/client.rs"]
mod tests;
