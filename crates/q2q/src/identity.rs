//! Versioned on-disk client identity. The private seed never enters the wire.

use ed25519_dalek::SigningKey;
use q2q_core::identity::{
  Audience, PeerId, TICKET_VERSION, Ticket, TicketBody, generate_keypair, keypair_from_seed,
  public_key_of, sign_message,
};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;

const MAGIC: &[u8; 8] = b"Q2Q-ID1\n";
const FILE_LEN: usize = MAGIC.len() + 32;

#[derive(Clone)]
pub(super) struct Identity {
  key: SigningKey,
  peer_id: PeerId,
}

impl Identity {
  pub(super) fn load_or_create(path: &Path) -> Result<Self, IdentityError> {
    match File::open(path) {
      Ok(file) => Self::read(file),
      Err(e) if e.kind() == io::ErrorKind::NotFound => Self::create(path),
      Err(e) => Err(IdentityError::Io(e)),
    }
  }

  fn read(mut file: File) -> Result<Self, IdentityError> {
    #[cfg(unix)]
    {
      use std::os::unix::fs::PermissionsExt;
      if file.metadata()?.permissions().mode() & 0o077 != 0 {
        return Err(IdentityError::InsecurePermissions);
      }
    }

    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    if bytes.len() != FILE_LEN || !bytes.starts_with(MAGIC) {
      return Err(IdentityError::InvalidFormat);
    }
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&bytes[MAGIC.len()..]);
    let (key, peer_id) = keypair_from_seed(&seed);
    Ok(Self { key, peer_id })
  }

  fn create(path: &Path) -> Result<Self, IdentityError> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
      fs::create_dir_all(parent)?;
    }
    let (key, peer_id) = generate_keypair().map_err(IdentityError::Random)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
      use std::os::unix::fs::OpenOptionsExt;
      options.mode(0o600);
    }
    let mut file = match options.open(path) {
      Ok(file) => file,
      // Another process created it after our first open. Preserve that key.
      Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Self::read(File::open(path)?),
      Err(e) => return Err(IdentityError::Io(e)),
    };
    file.write_all(MAGIC)?;
    file.write_all(&key.to_bytes())?;
    file.sync_all()?;
    Ok(Self { key, peer_id })
  }

  pub(super) fn peer_id(&self) -> &PeerId {
    &self.peer_id
  }

  pub(super) fn public_key(&self) -> [u8; 32] {
    public_key_of(&self.key)
  }

  pub(super) fn ticket(&self, audience: Audience, now: u64, cid: Option<String>) -> TicketBody {
    let mut ticket = Ticket {
      v: TICKET_VERSION,
      iss: self.peer_id.clone(),
      public_key: self.public_key(),
      iat: now,
      exp: now + 300,
      aud: audience.to_wire(),
      tgt: None,
      ih: None,
      cid,
      sig: [0; 64],
    };
    ticket.sign(&self.key);
    ticket.to_body()
  }

  pub(super) fn connection_ticket(
    &self,
    punch_id: &str,
    target: &PeerId,
    info_hash: [u8; 32],
    connection_id: &str,
    now: u64,
  ) -> TicketBody {
    let mut ticket = Ticket {
      v: TICKET_VERSION,
      iss: self.peer_id.clone(),
      public_key: self.public_key(),
      iat: now,
      exp: now + 60,
      aud: Audience::Punch(punch_id.to_owned()).to_wire(),
      tgt: Some(target.clone()),
      ih: Some(info_hash),
      cid: Some(connection_id.to_owned()),
      sig: [0; 64],
    };
    ticket.sign(&self.key);
    ticket.to_body()
  }

  pub(super) fn sign(&self, bytes: &[u8]) -> [u8; 64] {
    sign_message(&self.key, bytes)
  }
}

#[derive(Debug)]
pub enum IdentityError {
  Io(io::Error),
  Random(getrandom::Error),
  InvalidFormat,
  InsecurePermissions,
}

impl fmt::Display for IdentityError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::Io(e) => write!(f, "identity file I/O failed: {e}"),
      Self::Random(e) => write!(f, "identity generation failed: {e}"),
      Self::InvalidFormat => f.write_str("identity file has an unsupported or incomplete format"),
      Self::InsecurePermissions => f.write_str("identity file is readable by another user"),
    }
  }
}

impl std::error::Error for IdentityError {
  fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
    match self {
      Self::Io(e) => Some(e),
      Self::Random(e) => Some(e),
      _ => None,
    }
  }
}

impl From<io::Error> for IdentityError {
  fn from(value: io::Error) -> Self {
    Self::Io(value)
  }
}
