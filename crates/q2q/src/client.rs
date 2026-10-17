//! Client construction and identity lifecycle.

pub use crate::control::{ControlError, ControlSession};
pub use crate::peer::PeerSession;
pub use crate::publication::Publication;
pub use crate::transfer::{Transfer, TransferError};

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

  /// Validate the configuration and load or create the persistent identity.
  /// No connection to Track or Punch is opened yet.
  pub async fn build(self) -> Result<Client, BuildError> {
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

  /// Download from complete peers in Track order, switching on peer failure.
  /// `wait` succeeds only after every block has
  /// passed verification; existing destination files are never overwritten.
  pub async fn download(
    &self,
    seed: &q2q_core::seed::Seed,
    path: impl Into<PathBuf>,
  ) -> Result<Transfer, TransferError> {
    crate::transfer::check_seed(seed)?;
    let path = path.into();
    if tokio::fs::try_exists(&path).await? {
      return Err(
        std::io::Error::new(
          std::io::ErrorKind::AlreadyExists,
          "download destination already exists",
        )
        .into(),
      );
    }
    let session = self.connect().await?;
    let seed = seed.clone();
    let candidates = session.query(&seed.info_hash()).await?;
    let targets: Vec<_> = candidates
      .peers
      .into_iter()
      .filter(|peer| peer.complete && peer.peer_id != self.peer_id().as_str())
      .collect();
    if targets.is_empty() {
      return Err(TransferError::NoPeers);
    }
    let task = tokio::spawn(async move {
      let download = async {
        let mut store = None;
        let mut last_error = TransferError::NoPeers;
        for target in targets {
          let peer = match tokio::time::timeout(
            std::time::Duration::from_secs(45),
            session.dial_peer(&target, seed.info_hash()),
          )
          .await
          {
            Ok(Ok(peer)) => peer,
            Ok(Err(error)) => {
              last_error = error.into();
              continue;
            }
            Err(_) => {
              last_error = TransferError::Timeout;
              continue;
            }
          };
          let destination = match &store {
            Some(store) => Arc::clone(store),
            None => {
              let destination = crate::transfer::create_destination(&seed, &path).await?;
              store = Some(destination.clone());
              destination
            }
          };
          match peer.download_into(&seed, destination).await {
            Ok(()) => {
              crate::transfer::sync_destination(&path).await?;
              return Ok(path);
            }
            Err(error) if error.peer_retryable() => {
              if crate::transfer::destination_complete(store.as_ref().expect("destination exists"))?
              {
                crate::transfer::sync_destination(&path).await?;
                return Ok(path);
              }
              last_error = error;
            }
            Err(error) => return Err(error),
          }
        }
        Err(last_error)
      };
      tokio::select! {
        result = download => result,
        result = crate::publication::keep_online(&session) => Err(TransferError::Control(result)),
      }
    });
    Ok(Transfer { task: Some(task) })
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
  Storage(io::Error),
  Identity(crate::identity::IdentityError),
  IdentityTask(tokio::task::JoinError),
}

impl fmt::Display for BuildError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::Missing(field) => write!(f, "missing client setting: {field}"),
      Self::InvalidTrackUrl(e) => write!(f, "invalid Track URL: {e}"),
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
mod tests {
  use super::*;

  #[tokio::test]
  async fn identity_survives_rebuilding_the_client() {
    let dir = tempfile::tempdir().unwrap();
    let identity_file = dir.path().join("identity");
    let storage_dir = dir.path().join("storage");
    let make_client = || {
      builder()
        .track_url("https://track.example.net")
        .identity_file(&identity_file)
        .storage_dir(&storage_dir)
    };

    let first = make_client().build().await.unwrap();
    let peer_id = first.peer_id().clone();
    let public_key = first.public_key();
    drop(first);

    let second = make_client().build().await.unwrap();
    assert_eq!(second.peer_id(), &peer_id);
    assert_eq!(second.public_key(), public_key);
  }

  #[tokio::test]
  async fn corrupt_identity_is_rejected_without_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let identity_file = dir.path().join("identity");
    std::fs::write(&identity_file, b"broken").unwrap();
    #[cfg(unix)]
    {
      use std::os::unix::fs::PermissionsExt;
      std::fs::set_permissions(&identity_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    let result = builder()
      .track_url("https://track.example.net")
      .identity_file(&identity_file)
      .storage_dir(dir.path().join("storage"))
      .build()
      .await;
    assert!(matches!(
      result,
      Err(BuildError::Identity(
        crate::identity::IdentityError::InvalidFormat
      ))
    ));
    assert_eq!(std::fs::read(&identity_file).unwrap(), b"broken");
  }
}
