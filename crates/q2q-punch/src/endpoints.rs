//! Punch endpoint identity and connector capabilities, shared by native and Wasm hosts.
//!
//! Sorting makes list order irrelevant to signed identity comparisons. Connector
//! preference and capacity filtering belong to callers, not to this wire contract.

use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use serde::{Deserialize, Deserializer, Serialize};
use url::{Host, Url};

pub const MAX_ENDPOINTS: usize = 8;
pub const MAX_ENDPOINT_LENGTH: usize = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointError {
  InvalidEndpoint,
  InvalidCount,
  DuplicateEndpoint,
  InvalidSchemes,
}
impl std::fmt::Display for EndpointError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.write_str(match self {
      Self::InvalidEndpoint => "invalid Punch endpoint",
      Self::InvalidCount => "Punch requires 1–8 endpoints",
      Self::DuplicateEndpoint => "duplicate Punch endpoint",
      Self::InvalidSchemes => "invalid Punch scheme capabilities",
    })
  }
}
impl std::error::Error for EndpointError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PunchScheme {
  Quic,
  Ws,
  Wss,
}

/// Validated, canonical and sorted identity. Duplicate URLs are an error, not a merge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<String>", into = "Vec<String>")]
pub struct PunchEndpoints(Vec<String>);

impl TryFrom<Vec<String>> for PunchEndpoints {
  type Error = EndpointError;
  fn try_from(values: Vec<String>) -> Result<Self, Self::Error> {
    if !(1..=MAX_ENDPOINTS).contains(&values.len()) {
      return Err(EndpointError::InvalidCount);
    }
    let mut endpoints = values
      .iter()
      .map(|value| canonical_endpoint(value))
      .collect::<Result<Vec<_>, _>>()?;
    endpoints.sort();
    if endpoints.windows(2).any(|pair| pair[0] == pair[1]) {
      return Err(EndpointError::DuplicateEndpoint);
    }
    Ok(Self(endpoints))
  }
}
impl From<PunchEndpoints> for Vec<String> {
  fn from(value: PunchEndpoints) -> Self {
    value.0
  }
}
impl PunchEndpoints {
  pub fn as_slice(&self) -> &[String] {
    &self.0
  }
}

/// List-only assignment shape for the coordinated control-plane migration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PunchAssignment {
  #[serde(deserialize_with = "node_id")]
  pub punch_id: String,
  pub endpoints: PunchEndpoints,
  #[serde(deserialize_with = "public_key")]
  pub public_key: String,
}

/// Signed identity fields. JWT signature/issuer/scope checks remain Registry's job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PunchIdentity {
  pub endpoints: PunchEndpoints,
  #[serde(deserialize_with = "public_key")]
  pub public_key: String,
  #[serde(deserialize_with = "identity_version")]
  pub identity_version: u64,
}

fn node_id<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
  let value = String::deserialize(deserializer)?;
  if value.is_empty()
    || value.len() > 128
    || !value
      .bytes()
      .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
  {
    return Err(serde::de::Error::custom("invalid Punch ID"));
  }
  Ok(value)
}
fn public_key<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
  let value = String::deserialize(deserializer)?;
  if value.len() != 44
    || !B64
      .decode(&value)
      .is_ok_and(|bytes| bytes.len() == 32 && B64.encode(bytes) == value)
  {
    return Err(serde::de::Error::custom("invalid Punch public key"));
  }
  Ok(value)
}
fn identity_version<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
  let value = u64::deserialize(deserializer)?;
  if !(1..=9_007_199_254_740_991).contains(&value) {
    return Err(serde::de::Error::custom("invalid Punch identity version"));
  }
  Ok(value)
}

/// Announce's `punch_schemes` field; a set, never an implicit preference order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<PunchScheme>", into = "Vec<PunchScheme>")]
pub struct PunchSchemes(Vec<PunchScheme>);
impl TryFrom<Vec<PunchScheme>> for PunchSchemes {
  type Error = EndpointError;
  fn try_from(mut values: Vec<PunchScheme>) -> Result<Self, Self::Error> {
    if !(1..=3).contains(&values.len()) {
      return Err(EndpointError::InvalidSchemes);
    }
    values.sort();
    if values.windows(2).any(|pair| pair[0] == pair[1]) {
      return Err(EndpointError::InvalidSchemes);
    }
    Ok(Self(values))
  }
}
impl From<PunchSchemes> for Vec<PunchScheme> {
  fn from(value: PunchSchemes) -> Self {
    value.0
  }
}
impl PunchSchemes {
  pub fn as_slice(&self) -> &[PunchScheme] {
    &self.0
  }
}

/// No network/DNS lookup, and no input URL in errors (queries may contain secrets).
pub fn canonical_endpoint(value: &str) -> Result<String, EndpointError> {
  let invalid = EndpointError::InvalidEndpoint;
  // Match JS string.length (UTF-16), including the bound after URL serialization.
  if value.encode_utf16().count() > MAX_ENDPOINT_LENGTH
    || value
      .chars()
      .any(|c| c <= ' ' || matches!(c, '\u{7f}' | '\\' | '#'))
  {
    return Err(invalid);
  }
  let (scheme, rest) = value.split_once("://").ok_or(invalid)?;
  let scheme = scheme.to_ascii_lowercase();
  if !matches!(scheme.as_str(), "quic" | "wss" | "ws") {
    return Err(invalid);
  }
  let authority = rest.split(['/', '?', '#']).next().ok_or(invalid)?;
  if authority.is_empty() || authority.contains('@') || authority.ends_with(':') {
    return Err(invalid);
  }
  // Non-special QUIC URLs otherwise have different host/IDNA rules. Use the
  // same WHATWG special-authority parser as TS, then retain QUIC's explicit port.
  let parse_scheme = if scheme == "quic" { "https" } else { &scheme };
  let url = Url::parse(&format!("{parse_scheme}://{rest}")).map_err(|_| invalid)?;
  if url.host_str().is_none() || !url.username().is_empty() || url.password().is_some() {
    return Err(invalid);
  }
  let canonical = if scheme == "quic" {
    let port = authority
      .rsplit_once(':')
      .and_then(|(_, port)| port.parse::<u16>().ok())
      .filter(|port| *port > 0)
      .ok_or(invalid)?;
    if url.path() != "/" || value.contains('?') {
      return Err(invalid);
    }
    format!("quic://{}:{port}", url.host_str().ok_or(invalid)?)
  } else {
    if url.port() == Some(0) {
      return Err(invalid);
    }
    if scheme == "ws"
      && !match url.host() {
        Some(Host::Domain("localhost")) => true,
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
      }
    {
      return Err(invalid);
    }
    url.to_string()
  };
  if canonical.encode_utf16().count() > MAX_ENDPOINT_LENGTH {
    return Err(invalid);
  }
  Ok(canonical)
}
