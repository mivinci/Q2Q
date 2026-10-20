//! The server's TLS identity.
//!
//! Clients anchor on the key Track hands out as `punch_public_key` when they
//! announce or query: connecting to a Punch Server means checking that the
//! certificate in front of you carries exactly that key. So the raw public key
//! is exposed here deliberately — if it and the registered one ever disagree,
//! server authentication is theatre.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use quinn::ServerConfig;
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, PKCS_ED25519};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CertError {
  Generate(String),
  /// The key did not come out as a 32-byte Ed25519 key.
  BadPublicKey,
}

impl std::fmt::Display for CertError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      CertError::Generate(e) => write!(f, "certificate: {e}"),
      CertError::BadPublicKey => write!(f, "not a 32-byte ed25519 public key"),
    }
  }
}

impl std::error::Error for CertError {}

pub struct PunchIdentity {
  cert_der: Vec<u8>,
  key_der: Vec<u8>,
  public_key: [u8; 32],
}

impl PunchIdentity {
  /// A self-signed Ed25519 certificate. Development and tests; production wants
  /// one from a real CA, loaded with `from_pem`.
  pub fn generate(punch_id: &str) -> Result<PunchIdentity, CertError> {
    let key_pair =
      KeyPair::generate_for(&PKCS_ED25519).map_err(|e| CertError::Generate(e.to_string()))?;
    Self::with_key(punch_id, key_pair)
  }

  fn with_key(punch_id: &str, key_pair: KeyPair) -> Result<PunchIdentity, CertError> {
    let _ = raw_ed25519(&key_pair)?;
    let mut params =
      CertificateParams::new(Vec::new()).map_err(|e| CertError::Generate(e.to_string()))?;
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, punch_id);
    params.distinguished_name = dn;

    let cert = params
      .self_signed(&key_pair)
      .map_err(|e| CertError::Generate(e.to_string()))?;
    let der = key_pair.serialize_der();
    let public_key = raw_ed25519(&key_pair)?;

    Ok(PunchIdentity {
      cert_der: cert.der().to_vec(),
      key_der: der,
      public_key,
    })
  }

  pub fn from_pem(cert_pem: &str, key_pem: &str) -> Result<PunchIdentity, CertError> {
    let cert = CertificateDer::from_pem_slice(cert_pem.as_bytes())
      .map_err(|e| CertError::Generate(format!("cert pem: {e}")))?;
    let key = PrivateKeyDer::from_pem_slice(key_pem.as_bytes())
      .map_err(|e| CertError::Generate(format!("key pem: {e}")))?;
    // re-derive the public key from the private half rather than parsing the
    // certificate's SPKI, which would mean hand-rolling ASN.1
    let key_pair =
      KeyPair::from_pem(key_pem).map_err(|e| CertError::Generate(format!("key: {e}")))?;
    let public_key = raw_ed25519(&key_pair)?;

    let identity = PunchIdentity {
      cert_der: cert.to_vec(),
      key_der: key.secret_der().to_vec(),
      public_key,
    };
    identity.tls_server_config()?;
    Ok(identity)
  }

  /// The 32 bytes that must match the `punch_public_key` registered at Track.
  pub fn public_key(&self) -> [u8; 32] {
    self.public_key
  }

  pub fn server_config(&self) -> Result<ServerConfig, CertError> {
    let tls = self.tls_server_config()?;
    let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls)
      .map_err(|e| CertError::Generate(e.to_string()))?;
    Ok(ServerConfig::with_crypto(Arc::new(crypto)))
  }

  pub fn tls_server_config(&self) -> Result<rustls::ServerConfig, CertError> {
    let chain = vec![CertificateDer::from(self.cert_der.clone())];
    let key = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(self.key_der.clone()));
    rustls::ServerConfig::builder_with_provider(Arc::new(
      rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| CertError::Generate(e.to_string()))?
    .with_no_client_auth()
    .with_single_cert(chain, key)
    .map_err(|e| CertError::Generate(e.to_string()))
  }
}

/// Load the identity from disk, or mint one and keep it there.
///
/// The public key is a long-lived identity: it is what gets registered at Track
/// and what clients check the certificate against. A restart must not change it,
/// so a generated key is written out rather than held in memory only.
pub fn load_or_create(
  cert_path: &Path,
  key_path: &Path,
  punch_id: &str,
) -> Result<PunchIdentity, CertError> {
  let cert_exists = cert_path
    .try_exists()
    .map_err(|e| CertError::Generate(e.to_string()))?;
  let key_exists = key_path
    .try_exists()
    .map_err(|e| CertError::Generate(e.to_string()))?;
  if cert_exists != key_exists {
    return Err(CertError::Generate(
      "incomplete identity files; refusing to replace an existing certificate or key".into(),
    ));
  }
  if cert_exists {
    return load_existing(cert_path, key_path);
  }

  let identity = PunchIdentity::generate(punch_id)?;
  identity.save(cert_path, key_path)?;
  tracing::info!(cert = %cert_path.display(), "minted an identity and saved it");
  Ok(identity)
}

pub fn load_existing(cert_path: &Path, key_path: &Path) -> Result<PunchIdentity, CertError> {
  let cert = std::fs::read_to_string(cert_path)
    .map_err(|e| CertError::Generate(format!("read {}: {e}", cert_path.display())))?;
  let key = std::fs::read_to_string(key_path)
    .map_err(|e| CertError::Generate(format!("read {}: {e}", key_path.display())))?;
  PunchIdentity::from_pem(&cert, &key)
}

/// Import a pre-generated Ed25519 PKCS#8 PEM key and issue its certificate.
pub fn initialize_from_key(
  cert_path: &Path,
  key_path: &Path,
  punch_id: &str,
  source: &Path,
) -> Result<PunchIdentity, CertError> {
  if cert_path
    .try_exists()
    .map_err(|e| CertError::Generate(e.to_string()))?
    || key_path
      .try_exists()
      .map_err(|e| CertError::Generate(e.to_string()))?
  {
    return Err(CertError::Generate(
      "import destinations must not already exist".into(),
    ));
  }
  let pem = std::fs::read_to_string(source)
    .map_err(|e| CertError::Generate(format!("read input key: {e}")))?;
  let pair =
    KeyPair::from_pem(&pem).map_err(|_| CertError::Generate("invalid input private PEM".into()))?;
  let identity = PunchIdentity::with_key(punch_id, pair)?;
  identity.tls_server_config()?;
  identity.save(cert_path, key_path)?;
  Ok(identity)
}

impl PunchIdentity {
  fn save(&self, cert_path: &Path, key_path: &Path) -> Result<(), CertError> {
    if cert_path == key_path {
      return Err(CertError::Generate(
        "certificate and key paths must differ".into(),
      ));
    }
    for path in [cert_path, key_path] {
      if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| CertError::Generate(e.to_string()))?;
      }
    }
    // Never overwrite an existing half, including partial identities after a failed write.
    // Set secret permissions at creation so there is no world-readable write window.
    write_new(key_path, &pem("PRIVATE KEY", &self.key_der))?;
    write_new(cert_path, &pem("CERTIFICATE", &self.cert_der))?;
    Ok(())
  }
}

fn write_new(path: &Path, value: &str) -> Result<(), CertError> {
  use std::io::Write;
  let mut options = std::fs::OpenOptions::new();
  options.write(true).create_new(true);
  #[cfg(unix)]
  {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
  }
  let mut file = options
    .open(path)
    .map_err(|e| CertError::Generate(format!("create {}: {e}", path.display())))?;
  file
    .write_all(value.as_bytes())
    .and_then(|_| file.sync_all())
    .map_err(|e| CertError::Generate(format!("write {}: {e}", path.display())))
}

/// PEM with 64-character lines, which is what both parsers expect.
fn pem(label: &str, der: &[u8]) -> String {
  let b64 = B64.encode(der);
  let mut out = format!("-----BEGIN {label}-----\n");
  for chunk in b64.as_bytes().chunks(64) {
    out.push_str(std::str::from_utf8(chunk).unwrap_or_default());
    out.push('\n');
  }
  out.push_str(&format!("-----END {label}-----\n"));
  out
}

/// rcgen hands out the public key as SPKI PEM; an Ed25519 SPKI is a 12-byte
/// prefix followed by the raw 32-byte key.
fn raw_ed25519(key_pair: &KeyPair) -> Result<[u8; 32], CertError> {
  if key_pair.algorithm() != &PKCS_ED25519 {
    return Err(CertError::BadPublicKey);
  }
  let spki = spki_der(&key_pair.public_key_pem())?;
  let tail = spki
    .get(spki.len().saturating_sub(32)..)
    .ok_or(CertError::BadPublicKey)?;
  tail.try_into().map_err(|_| CertError::BadPublicKey)
}

fn spki_der(pem: &str) -> Result<Vec<u8>, CertError> {
  let b64: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
  B64
    .decode(b64)
    .map_err(|e| CertError::Generate(format!("public key: {e}")))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn the_public_key_is_the_ed25519_key() {
    let id = PunchIdentity::generate("punch-1").unwrap();
    // 32 raw bytes, and it is what a peer would check against Track
    assert_eq!(id.public_key().len(), 32);
    assert_ne!(id.public_key(), [0u8; 32]);
  }

  #[test]
  fn a_pem_round_trip_keeps_the_same_key() {
    let id = PunchIdentity::generate("punch-1").unwrap();
    let cert_pem = pem("CERTIFICATE", &id.cert_der);
    let key_pem = pem("PRIVATE KEY", &id.key_der);

    let back = PunchIdentity::from_pem(&cert_pem, &key_pem).unwrap();
    assert_eq!(back.public_key(), id.public_key());
  }

  #[test]
  fn an_identity_survives_a_restart() {
    // the whole point: the public key is registered at Track, so it has to be
    // the same key after the process comes back
    let dir = tempfile::tempdir().unwrap();
    let cert = dir.path().join("cert.pem");
    let key = dir.path().join("key.pem");

    let first = load_or_create(&cert, &key, "punch-1").unwrap();
    assert!(cert.exists());
    let second = load_or_create(&cert, &key, "punch-1").unwrap();
    assert_eq!(first.public_key(), second.public_key());
  }

  #[test]
  fn import_preserves_key_and_partial_identity_is_not_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let original = PunchIdentity::generate("source").unwrap();
    let source = dir.path().join("source.pem");
    std::fs::write(&source, pem("PRIVATE KEY", &original.key_der)).unwrap();
    let cert = dir.path().join("cert.pem");
    let key = dir.path().join("key.pem");
    let imported = initialize_from_key(&cert, &key, "punch-1", &source).unwrap();
    assert_eq!(imported.public_key(), original.public_key());
    assert_eq!(
      load_existing(&cert, &key).unwrap().public_key(),
      original.public_key()
    );
    assert!(initialize_from_key(&cert, &key, "punch-1", &source).is_err());
    std::fs::remove_file(&cert).unwrap();
    let before = std::fs::read(&key).unwrap();
    assert!(load_or_create(&cert, &key, "punch-1").is_err());
    assert_eq!(std::fs::read(&key).unwrap(), before);
    assert!(!cert.exists());
    #[cfg(unix)]
    {
      use std::os::unix::fs::PermissionsExt;
      assert_eq!(
        std::fs::metadata(&key).unwrap().permissions().mode() & 0o777,
        0o600
      );
    }
  }

  #[test]
  fn mismatched_certificate_and_key_are_rejected() {
    let a = PunchIdentity::generate("a").unwrap();
    let b = PunchIdentity::generate("b").unwrap();
    assert!(
      PunchIdentity::from_pem(
        &pem("CERTIFICATE", &a.cert_der),
        &pem("PRIVATE KEY", &b.key_der)
      )
      .is_err()
    );
  }

  #[test]
  fn two_servers_do_not_share_an_identity() {
    let a = PunchIdentity::generate("punch-1").unwrap();
    let b = PunchIdentity::generate("punch-2").unwrap();
    assert_ne!(a.public_key(), b.public_key());
  }

  #[test]
  fn a_server_config_can_be_built() {
    let id = PunchIdentity::generate("punch-1").unwrap();
    assert!(id.server_config().is_ok());
  }
}
