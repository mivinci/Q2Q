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

    Ok(PunchIdentity {
      cert_der: cert.to_vec(),
      key_der: key.secret_der().to_vec(),
      public_key,
    })
  }

  /// The 32 bytes that must match the `punch_public_key` registered at Track.
  pub fn public_key(&self) -> [u8; 32] {
    self.public_key
  }

  pub fn server_config(&self) -> Result<ServerConfig, CertError> {
    let chain = vec![CertificateDer::from(self.cert_der.clone())];
    let key = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(self.key_der.clone()));
    ServerConfig::with_single_cert(chain, key).map_err(|e| CertError::Generate(e.to_string()))
  }
}

/// rcgen hands out the public key as SPKI PEM; an Ed25519 SPKI is a 12-byte
/// prefix followed by the raw 32-byte key.
fn raw_ed25519(key_pair: &KeyPair) -> Result<[u8; 32], CertError> {
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
    let cert_pem = pem_encode("CERTIFICATE", &id.cert_der);
    let key_pem = pem_encode("PRIVATE KEY", &id.key_der);

    let back = PunchIdentity::from_pem(&cert_pem, &key_pem).unwrap();
    assert_eq!(back.public_key(), id.public_key());
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

  fn pem_encode(label: &str, der: &[u8]) -> String {
    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, der);
    let mut out = format!("-----BEGIN {label}-----\n");
    for chunk in b64.as_bytes().chunks(64) {
      out.push_str(std::str::from_utf8(chunk).unwrap());
      out.push('\n');
    }
    out.push_str(&format!("-----END {label}-----\n"));
    out
  }
}
