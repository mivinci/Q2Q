//! Authenticated Registry discovery with an all-pages, lease-bounded in-memory cache.
use crate::directory::{DirectoryError, DirectorySnapshot, PunchDirectory};
use base64::{
  Engine as _,
  engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use q2q_primitives::identity::PeerId;
use q2q_track::state::PunchAssignment;
use reqwest::{
  Client, StatusCode,
  header::{AUTHORIZATION, HeaderMap, HeaderValue},
};
use serde::Deserialize;
use std::{
  collections::HashMap,
  io::Read,
  path::PathBuf,
  sync::{Arc, RwLock},
  time::Duration,
};
use tokio::{sync::watch, time::Instant};
use url::Url;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
  pub url: String,
  pub token_file: Option<PathBuf>,
  pub region: Option<String>,
  #[serde(default = "default_accepting")]
  pub accepting_new_connections: bool,
}

fn default_accepting() -> bool {
  true
}

#[derive(Deserialize)]
struct TokenClaims {
  iss: String,
  aud: String,
  sub: String,
  service_type: String,
  advertise_url: String,
  identity_version: u64,
  iat: u64,
  exp: u64,
  jti: String,
  scope: Vec<String>,
}

#[derive(Deserialize)]
struct Page {
  server_time: u64,
  nodes: Vec<Node>,
  next_cursor: Option<String>,
}
#[derive(Deserialize)]
struct Node {
  punch_id: String,
  url: String,
  public_key: String,
  identity_version: u64,
  accepting_new_connections: bool,
  last_seen: u64,
  expires_at: u64,
  metrics: Option<Metrics>,
}
#[derive(Deserialize)]
struct Metrics {
  quic_connections: Option<u64>,
  quic_connection_capacity: Option<u64>,
  websocket_connections: Option<u64>,
  websocket_connection_capacity: Option<u64>,
}
struct LeasedNode {
  assignment: PunchAssignment,
  assignable: bool,
  expires_at: Instant,
}
struct Cache {
  nodes: Vec<LeasedNode>,
  expires_at: Instant,
}
pub struct RegistryPunchDirectory {
  client: Client,
  endpoint: Url,
  cache: RwLock<Option<Cache>>,
  // Keep accepted identities even when a later snapshot omits or expires a node.
  identities: RwLock<HashMap<String, (u64, PunchAssignment)>>,
  token_expires_at: u64,
  node_id: String,
  report_endpoint: Url,
  report_enabled: bool,
  region: Option<String>,
  accepting: bool,
}

fn endpoint(value: &str) -> Result<Url, &'static str> {
  let mut url = Url::parse(value).map_err(|_| "invalid Registry URL")?;
  let host = url.host_str().ok_or("Registry URL must include a host")?;
  let loopback = host == "localhost"
    || host
      .trim_matches(['[', ']'])
      .parse::<std::net::IpAddr>()
      .is_ok_and(|ip| ip.is_loopback());
  if !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
    || !url.username().is_empty()
    || url.password().is_some()
    || url.query().is_some()
    || url.fragment().is_some()
  {
    return Err(
      "Registry URL requires HTTPS (HTTP allowed only on loopback), without credentials, query or fragment",
    );
  }
  url.set_path(&format!(
    "{}/v1/discovery/punches",
    url.path().trim_end_matches('/')
  ));
  Ok(url)
}

impl RegistryPunchDirectory {
  pub fn prepare(config: &Config, env_token: Option<String>) -> Result<Arc<Self>, &'static str> {
    let endpoint = endpoint(&config.url)?;
    let token = match (&config.token_file, env_token) {
      (Some(_), Some(_)) => return Err("use either Q2Q_REGISTRY_TOKEN or registry.token_file"),
      (Some(path), None) => {
        let file = std::fs::File::open(path).map_err(|_| "cannot read Registry token file")?;
        let mut bytes = Vec::new();
        file
          .take(8194)
          .read_to_end(&mut bytes)
          .map_err(|_| "cannot read Registry token file")?;
        String::from_utf8(bytes).map_err(|_| "Registry token file is not UTF-8")?
      }
      (None, Some(value)) => value,
      (None, None) => return Err("Registry requires Q2Q_REGISTRY_TOKEN or registry.token_file"),
    };
    let token = token.trim();
    if token.is_empty() || token.len() > 8192 {
      return Err("invalid Registry token length");
    }
    let parts: Vec<_> = token.split('.').collect();
    if parts.len() != 3 {
      return Err("Registry token must be a JWT");
    }
    let claims: TokenClaims = URL_SAFE_NO_PAD
      .decode(parts[1])
      .ok()
      .and_then(|bytes| serde_json::from_slice(&bytes).ok())
      .ok_or("invalid Registry token identity claims")?;
    let now = std::time::SystemTime::now()
      .duration_since(std::time::UNIX_EPOCH)
      .map_err(|_| "system clock is before the Unix epoch")?
      .as_secs();
    if claims.iss != "q2q-registry"
      || claims.aud != "q2q-registry"
      || claims.sub.is_empty()
      || claims.sub.len() > 128
      || !claims
        .sub
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
      || claims.jti.is_empty()
      || claims.jti.len() > 128
      || claims.iat > claims.exp
      || claims.exp <= now
      || claims.exp > 9_007_199_254_740_991
      || claims.service_type != "track"
      || claims.identity_version == 0
      || claims.identity_version > 9_007_199_254_740_991
      || claims.scope.is_empty()
      || claims.scope.len() > 2
      || claims
        .scope
        .iter()
        .collect::<std::collections::HashSet<_>>()
        .len()
        != claims.scope.len()
      || !claims
        .scope
        .iter()
        .any(|scope| scope == "discovery:punch:read")
      || claims
        .scope
        .iter()
        .any(|scope| !["nodes:write:self", "discovery:punch:read"].contains(&scope.as_str()))
    {
      return Err("Registry token must identify this Track and grant discovery:punch:read");
    }
    let advertised =
      Url::parse(&claims.advertise_url).map_err(|_| "invalid Track advertised URL")?;
    if advertised.scheme() != "https"
      || advertised.host_str().is_none()
      || !advertised.username().is_empty()
      || advertised.password().is_some()
      || advertised.query().is_some()
      || advertised.fragment().is_some()
    {
      return Err("Track token requires a public HTTPS advertised URL");
    }
    if config.region.as_ref().is_some_and(|region| {
      region.is_empty()
        || region.len() > 32
        || !region.as_bytes()[0].is_ascii_alphanumeric()
        || !region
          .bytes()
          .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
    }) {
      return Err("Registry region must be a short ASCII region label");
    }
    let mut report_endpoint = endpoint.clone();
    report_endpoint.set_path(&format!(
      "{}/v1/nodes/self",
      endpoint
        .path()
        .strip_suffix("/v1/discovery/punches")
        .ok_or("invalid Registry endpoint")?
    ));
    let mut authorization = HeaderValue::from_str(&format!("Bearer {token}"))
      .map_err(|_| "invalid Registry authorization header")?;
    authorization.set_sensitive(true);
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, authorization);
    let mut builder = Client::builder()
      .default_headers(headers)
      .redirect(reqwest::redirect::Policy::none())
      .connect_timeout(Duration::from_secs(5))
      .timeout(Duration::from_secs(10));
    if endpoint.scheme() == "http" {
      builder = builder.no_proxy();
    }
    Ok(Arc::new(Self {
      client: builder
        .build()
        .map_err(|_| "cannot initialize Registry client")?,
      endpoint,
      cache: RwLock::new(None),
      identities: RwLock::new(HashMap::new()),
      token_expires_at: claims.exp,
      node_id: claims.sub,
      report_endpoint,
      report_enabled: claims.scope.iter().any(|scope| scope == "nodes:write:self"),
      region: config.region.clone(),
      accepting: config.accepting_new_connections,
    }))
  }

  pub async fn run(self: Arc<Self>, stop: watch::Receiver<bool>) {
    tokio::join!(
      self.clone().run_discovery(stop.clone()),
      self.run_report(stop)
    );
  }

  async fn run_report(self: Arc<Self>, mut stop: watch::Receiver<bool>) {
    if !self.report_enabled {
      tracing::info!("Registry Track reporting disabled: token grants discovery only");
      return;
    }
    let mut backoff = 1u64;
    let mut delay = Duration::ZERO;
    let mut previous = None;
    loop {
      if *stop.borrow() {
        break;
      }
      if !delay.is_zero() {
        tokio::select! { _ = stop.changed() => break, _ = tokio::time::sleep(delay) => {} }
      }
      let result =
        tokio::select! { _ = stop.changed() => break, result = self.report_once() => result };
      match result {
        Ok(()) => {
          if previous.is_some() || delay.is_zero() {
            tracing::info!(track = %self.node_id, "Registry Track reporting active");
          }
          previous = None;
          backoff = 1;
          delay = jitter(30);
        }
        Err(FetchError::Unauthorized | FetchError::Rejected) => {
          tracing::error!(
            "Registry Track reporting stopped; update credentials or configuration and restart"
          );
          break;
        }
        Err(error) => {
          let reason = error.message();
          if previous != Some(reason) {
            tracing::warn!(
              reason,
              "Registry Track report failed; retrying in background"
            );
          }
          previous = Some(reason);
          let retry = match error {
            FetchError::Temporary(value) => value.unwrap_or(0),
            _ => 0,
          };
          delay = jitter(backoff)
            .min(Duration::from_secs(30))
            .max(Duration::from_secs(retry));
          backoff = backoff.saturating_mul(2).min(30);
        }
      }
    }
  }

  async fn report_once(&self) -> Result<(), FetchError> {
    let body = serde_json::json!({ "version": env!("CARGO_PKG_VERSION"), "region": self.region,
      "accepting_new_connections": self.accepting, "metrics": null });
    let mut response = self
      .client
      .put(self.report_endpoint.clone())
      .json(&body)
      .send()
      .await
      .map_err(|_| FetchError::Network)?;
    match response.status() {
      StatusCode::OK => {}
      StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => return Err(FetchError::Unauthorized),
      StatusCode::TOO_MANY_REQUESTS => {
        return Err(FetchError::Temporary(
          response
            .headers()
            .get("Retry-After")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .map(|value| value.clamp(1, 60)),
        ));
      }
      status if status.is_server_error() => return Err(FetchError::Temporary(None)),
      _ => return Err(FetchError::Rejected),
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| FetchError::Network)? {
      if body.len() + chunk.len() > 16 * 1024 {
        return Err(FetchError::Invalid);
      }
      body.extend_from_slice(&chunk);
    }
    #[derive(Deserialize)]
    struct Ack {
      node_id: String,
      last_seen: u64,
      expires_at: u64,
      report_interval_seconds: u64,
    }
    let ack: Ack = serde_json::from_slice(&body).map_err(|_| FetchError::Invalid)?;
    if ack.node_id != self.node_id
      || ack.report_interval_seconds != 30
      || ack.expires_at <= ack.last_seen
      || ack.expires_at - ack.last_seen > 90
      || ack.expires_at > self.token_expires_at
    {
      return Err(FetchError::Invalid);
    }
    Ok(())
  }

  async fn run_discovery(self: Arc<Self>, mut stop: watch::Receiver<bool>) {
    let mut backoff = 1u64;
    let mut retry_delay = Duration::ZERO;
    let mut previous: Option<&'static str> = None;
    loop {
      if !retry_delay.is_zero() {
        tokio::select! { _ = stop.changed() => break, _ = tokio::time::sleep(retry_delay) => {} }
      }
      if *stop.borrow() {
        break;
      }
      let result = tokio::select! {
        _ = stop.changed() => break,
        value = tokio::time::timeout(Duration::from_secs(30), self.refresh()) => value.unwrap_or(Err(FetchError::Timeout)),
      };
      match result {
        Ok(()) => {
          if previous.is_some() || retry_delay.is_zero() {
            tracing::info!("Registry Punch discovery refreshed");
          }
          previous = None;
          backoff = 1;
          retry_delay = jitter(30);
        }
        Err(FetchError::Unauthorized) => {
          tracing::error!(
            "Registry rejected Track credentials; discovery refresh stopped until restart"
          );
          tokio::select! { _ = stop.changed() => break, _ = std::future::pending::<()>() => {} }
        }
        Err(error) => {
          let reason = error.message();
          if previous != Some(reason) {
            tracing::warn!(
              reason,
              "Registry discovery refresh failed; retaining unexpired cache"
            );
          }
          previous = Some(reason);
          let retry_after = match error {
            FetchError::Temporary(value) => value.unwrap_or(0),
            _ => 0,
          };
          retry_delay = jitter(backoff).max(Duration::from_secs(retry_after));
          backoff = backoff.saturating_mul(2).min(30);
        }
      }
    }
  }

  async fn refresh(&self) -> Result<(), FetchError> {
    let started = Instant::now();
    let mut cursor: Option<String> = None;
    let mut seen_cursors = std::collections::HashSet::new();
    let mut nodes = Vec::new();
    let mut snapshot_deadline: Option<Instant> = None;
    let mut page_count = 0usize;
    let mut seen_nodes = std::collections::HashSet::new();
    let mut identities = HashMap::new();
    loop {
      page_count += 1;
      if page_count > 100 {
        return Err(FetchError::Invalid);
      }
      let mut request = self
        .client
        .get(self.endpoint.clone())
        .query(&[("limit", "200")]);
      if let Some(after) = cursor.as_deref() {
        request = request.query(&[("after", after)]);
      }
      let response = request.send().await.map_err(|_| FetchError::Network)?;
      if response.status() == StatusCode::UNAUTHORIZED || response.status() == StatusCode::FORBIDDEN
      {
        return Err(FetchError::Unauthorized);
      }
      if !response.status().is_success() {
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
          let retry = response
            .headers()
            .get("Retry-After")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .map(|value| value.clamp(1, 60));
          return Err(FetchError::Temporary(retry));
        }
        return Err(if response.status().is_server_error() {
          FetchError::Temporary(None)
        } else {
          FetchError::Rejected
        });
      }
      if response
        .content_length()
        .is_some_and(|length| length > 1024 * 1024)
      {
        return Err(FetchError::Invalid);
      }
      let mut response = response;
      let mut body = Vec::new();
      while let Some(chunk) = response.chunk().await.map_err(|_| FetchError::Network)? {
        if body.len() + chunk.len() > 1024 * 1024 {
          return Err(FetchError::Invalid);
        }
        body.extend_from_slice(&chunk);
      }
      let page: Page = serde_json::from_slice(&body).map_err(|_| FetchError::Invalid)?;
      let now = Instant::now();
      let elapsed = now.saturating_duration_since(started);
      let global_ttl = Duration::from_secs(90).saturating_sub(elapsed);
      let token_ttl = Duration::from_secs(self.token_expires_at.saturating_sub(page.server_time))
        .saturating_sub(elapsed)
        .min(global_ttl);
      snapshot_deadline = Some(
        snapshot_deadline.map_or(now + global_ttl.min(token_ttl), |current| {
          current.min(now + global_ttl).min(now + token_ttl)
        }),
      );
      for node in page.nodes {
        if node.punch_id.is_empty()
          || node.punch_id.len() > 128
          || !node
            .punch_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
        {
          return Err(FetchError::Invalid);
        }
        if !seen_nodes.insert(node.punch_id.clone()) {
          return Err(FetchError::Invalid);
        }
        if node.expires_at <= page.server_time
          || node.last_seen > page.server_time
          || node.expires_at.saturating_sub(node.last_seen) > 90
          || node.expires_at - page.server_time > 90
        {
          return Err(FetchError::Invalid);
        }
        let remaining =
          Duration::from_secs(node.expires_at - page.server_time).saturating_sub(elapsed);
        if remaining.is_zero() {
          continue;
        }
        let key = STANDARD
          .decode(&node.public_key)
          .map_err(|_| FetchError::Invalid)?;
        if key.len() != 32
          || node.identity_version == 0
          || node.identity_version > 9_007_199_254_740_991
        {
          return Err(FetchError::Invalid);
        }
        let url = Url::parse(&node.url).map_err(|_| FetchError::Invalid)?;
        if url.scheme() != "quic"
          || url.host_str().is_none()
          || url.port().is_none()
          || !url.username().is_empty()
          || url.password().is_some()
          || url.query().is_some()
          || url.fragment().is_some()
          || !matches!(url.path(), "" | "/")
        {
          return Err(FetchError::Invalid);
        }
        let full = node.metrics.as_ref().is_some_and(|metrics| {
          transport_full(metrics.quic_connections, metrics.quic_connection_capacity)
            && transport_full(
              metrics.websocket_connections,
              metrics.websocket_connection_capacity,
            )
        });
        let assignment = PunchAssignment {
          punch_id: node.punch_id,
          url: node.url,
          public_key: key,
        };
        identities.insert(
          assignment.punch_id.clone(),
          (node.identity_version, assignment.clone()),
        );
        nodes.push(LeasedNode {
          assignment,
          assignable: node.accepting_new_connections && !full,
          expires_at: now + remaining,
        });
        if nodes.len() > 10_000 {
          return Err(FetchError::Invalid);
        }
      }
      cursor = page.next_cursor;
      match cursor.as_ref() {
        None => break,
        Some(value) if seen_cursors.insert(value.clone()) => {}
        Some(_) => return Err(FetchError::Invalid),
      }
    }
    let expires_at = snapshot_deadline.ok_or(FetchError::Invalid)?;
    if expires_at <= Instant::now() {
      return Err(FetchError::Invalid);
    }
    // Validate the whole response before changing either history or the cache.
    // Holding this guard through publication also prevents overlapping refreshes
    // from replacing a newer accepted identity with an older response.
    let mut observed = self
      .identities
      .write()
      .unwrap_or_else(std::sync::PoisonError::into_inner);
    for (id, (version, assignment)) in &identities {
      if let Some((previous_version, previous_assignment)) = observed.get(id)
        && (version < previous_version
          || (version == previous_version && assignment != previous_assignment))
      {
        return Err(FetchError::Invalid);
      }
    }
    observed.extend(identities);
    *self
      .cache
      .write()
      .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Cache { nodes, expires_at });
    Ok(())
  }
}

/// Rendezvous hashing keeps a Peer on the same Punch while the eligible set is
/// unchanged, and gives all Track replicas the same answer without shared counters.
fn rendezvous(nodes: &[LeasedNode], peer: &PeerId, now: Instant) -> Option<PunchAssignment> {
  nodes
    .iter()
    .filter(|node| node.assignable && node.expires_at > now)
    .max_by(|a, b| {
      let score = |node: &LeasedNode| {
        q2q_primitives::mac::sha256(
          b"q2q-punch-assignment-v1",
          &[
            peer.as_str().as_bytes(),
            b"\0",
            node.assignment.punch_id.as_bytes(),
          ],
        )
      };
      score(a)
        .cmp(&score(b))
        .then_with(|| a.assignment.punch_id.cmp(&b.assignment.punch_id))
    })
    .map(|node| node.assignment.clone())
}

impl PunchDirectory for RegistryPunchDirectory {
  fn assign(&self, peer: &PeerId) -> Result<Option<PunchAssignment>, DirectoryError> {
    let guard = self
      .cache
      .read()
      .unwrap_or_else(std::sync::PoisonError::into_inner);
    let cache = guard
      .as_ref()
      .filter(|cache| cache.expires_at > Instant::now())
      .ok_or(DirectoryError::Unavailable)?;
    Ok(rendezvous(&cache.nodes, peer, Instant::now()))
  }

  fn snapshot(&self) -> Result<DirectorySnapshot, DirectoryError> {
    let guard = self
      .cache
      .read()
      .unwrap_or_else(std::sync::PoisonError::into_inner);
    let cache = guard
      .as_ref()
      .filter(|cache| cache.expires_at > Instant::now())
      .ok_or(DirectoryError::Unavailable)?;
    let now = Instant::now();
    let mut nodes = HashMap::new();
    let mut assignable = Vec::new();
    for node in &cache.nodes {
      if node.expires_at <= now {
        continue;
      }
      let id = node.assignment.punch_id.clone();
      if node.assignable {
        assignable.push(id.clone());
      }
      nodes.insert(id, node.assignment.clone());
    }
    assignable.sort();
    Ok(DirectorySnapshot {
      nodes,
      assignable,
      strict_identity: true,
    })
  }

  fn requires_matching_identity(&self) -> bool {
    true
  }
}

#[derive(Clone, Copy)]
enum FetchError {
  Unauthorized,
  Network,
  Timeout,
  Temporary(Option<u64>),
  Rejected,
  Invalid,
}
impl FetchError {
  fn message(self) -> &'static str {
    match self {
      Self::Unauthorized => "Registry rejected Track credentials",
      Self::Network => "Registry network failure",
      Self::Timeout => "Registry discovery refresh timed out",
      Self::Temporary(_) => "Registry temporarily unavailable",
      Self::Rejected => "Registry rejected discovery request",
      Self::Invalid => "Registry returned invalid discovery data",
    }
  }
}
fn transport_full(connections: Option<u64>, capacity: Option<u64>) -> bool {
  matches!((connections, capacity), (Some(connections), Some(capacity)) if connections >= capacity)
}

fn jitter(seconds: u64) -> Duration {
  let mut random = [0];
  let _ = getrandom::fill(&mut random);
  Duration::from_millis(seconds * (800 + u64::from(random[0] % 41) * 10))
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
