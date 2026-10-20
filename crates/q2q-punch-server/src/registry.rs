//! Optional Registry reporting. Credentials are never logged or sent on redirects.
use crate::server::{CONNECTION_LIMIT, State};
use base64::{
  Engine as _,
  engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use reqwest::{
  Client, StatusCode,
  header::{AUTHORIZATION, HeaderMap, HeaderValue},
};
use serde::{Deserialize, Serialize};
use std::{io::Read, path::PathBuf, sync::Arc, time::Duration};
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

pub struct Reporter {
  client: Client,
  endpoint: Url,
  node_id: String,
  region: Option<String>,
  accepting: bool,
}

#[derive(Deserialize)]
struct TokenIdentity {
  sub: String,
  service_type: String,
  public_key: String,
  advertise_url: String,
  identity_version: u64,
  scope: Vec<String>,
}

pub fn endpoint(base: &str) -> Result<Url, &'static str> {
  let mut url = Url::parse(base).map_err(|_| "invalid Registry URL")?;
  let local = url.host_str().is_some_and(|host| {
    host == "localhost"
      || host
        .trim_matches(['[', ']'])
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
  });
  if url.host_str().is_none()
    || !(url.scheme() == "https" || (url.scheme() == "http" && local))
    || !url.username().is_empty()
    || url.password().is_some()
    || url.query().is_some()
    || url.fragment().is_some()
  {
    return Err(
      "Registry URL requires HTTPS (HTTP allowed only on loopback), without credentials, query or fragment",
    );
  }
  let path = format!("{}/v1/nodes/self", url.path().trim_end_matches('/'));
  url.set_path(&path);
  Ok(url)
}

impl Reporter {
  pub fn prepare(
    config: &Config,
    node_id: &str,
    public_key: [u8; 32],
    env_token: Option<String>,
  ) -> Result<Self, &'static str> {
    let endpoint = endpoint(&config.url)?;
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
    let token = match (&config.token_file, env_token) {
      (Some(_), Some(_)) => return Err("use either Q2Q_REGISTRY_TOKEN or registry.token_file"),
      (None, Some(value)) => value,
      (Some(path), None) => {
        let file = std::fs::File::open(path).map_err(|_| "cannot read Registry token file")?;
        let mut bytes = Vec::new();
        file
          .take(8194)
          .read_to_end(&mut bytes)
          .map_err(|_| "cannot read Registry token file")?;
        String::from_utf8(bytes).map_err(|_| "Registry token file is not UTF-8")?
      }
      (None, None) => return Err("Registry requires Q2Q_REGISTRY_TOKEN or registry.token_file"),
    };
    let token = token.trim();
    if token.is_empty() || token.len() > 8192 {
      return Err("invalid Registry token length");
    }
    // This is a local configuration check, not signature verification. Registry
    // verifies the JWT; checking the loaded identity here prevents wrong-ID/key reports.
    let parts: Vec<_> = token.split('.').collect();
    if parts.len() != 3 {
      return Err("Registry token must be a JWT");
    }
    let claims: TokenIdentity = URL_SAFE_NO_PAD
      .decode(parts[1])
      .ok()
      .and_then(|bytes| serde_json::from_slice(&bytes).ok())
      .ok_or("invalid Registry token identity claims")?;
    let advertised = Url::parse(&claims.advertise_url).map_err(|_| "invalid token Punch URL")?;
    if claims.sub != node_id
      || claims.service_type != "punch"
      || claims.public_key != STANDARD.encode(public_key)
      || claims.identity_version == 0
      || claims.identity_version > 9_007_199_254_740_991
      || claims.scope != ["nodes:write:self"]
      || advertised.scheme() != "quic"
      || advertised.host_str().is_none()
      || advertised.port().is_none()
      || !advertised.username().is_empty()
      || advertised.password().is_some()
      || advertised.query().is_some()
      || advertised.fragment().is_some()
      || !matches!(advertised.path(), "" | "/")
    {
      return Err("Registry token identity does not match the running Punch configuration");
    }
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
    // Local development credentials must stay on loopback, even if an HTTP
    // proxy is configured for public network access.
    if endpoint.scheme() == "http" {
      builder = builder.no_proxy();
    }
    let client = builder
      .build()
      .map_err(|_| "cannot initialize Registry HTTPS client")?;
    Ok(Self {
      client,
      endpoint,
      node_id: node_id.into(),
      region: config.region.clone(),
      accepting: config.accepting_new_connections,
    })
  }

  pub async fn run(self, state: Arc<State>, mut stop: watch::Receiver<bool>) {
    let mut backoff = 1u64;
    let mut previous: Option<&'static str> = None;
    let mut last_warning = Instant::now();
    let mut first = true;
    loop {
      if *stop.borrow() {
        break;
      }
      let result = tokio::select! {
        _ = stop.changed() => break,
        result = self.send(snapshot(&state, self.region.clone(), self.accepting)) => result,
      };
      let delay = match result {
        Outcome::Success => {
          if first || previous.is_some() {
            tracing::info!(punch = %self.node_id, "Registry reporting active");
          }
          first = false;
          previous = None;
          backoff = 1;
          jitter(30)
        }
        Outcome::Retry(reason, retry_after) => {
          if previous != Some(reason) || last_warning.elapsed() >= Duration::from_secs(60) {
            tracing::warn!(punch = %self.node_id, reason, "Registry report failed; retrying in background");
            last_warning = Instant::now();
          }
          previous = Some(reason);
          let delay = jitter(backoff)
            .min(Duration::from_secs(30))
            .max(Duration::from_secs(retry_after.unwrap_or(0)));
          backoff = backoff.saturating_mul(2).min(30);
          delay
        }
        Outcome::Stop(reason) => {
          tracing::error!(punch = %self.node_id, reason, "Registry reporting stopped; update credentials or configuration and restart Punch");
          break;
        }
      };
      tokio::select! { _ = stop.changed() => break, _ = tokio::time::sleep(delay) => {} }
    }
  }

  async fn send(&self, body: Report) -> Outcome {
    let mut response = match self
      .client
      .put(self.endpoint.clone())
      .json(&body)
      .send()
      .await
    {
      Ok(value) => value,
      Err(_) => return Outcome::Retry("network or request timeout", None),
    };
    let status = response.status();
    if status != StatusCode::OK {
      return classify(
        status,
        response
          .headers()
          .get("Retry-After")
          .and_then(|h| h.to_str().ok()),
      );
    }
    let mut bytes = Vec::new();
    loop {
      match response.chunk().await {
        Ok(Some(chunk)) if bytes.len() + chunk.len() <= 16384 => bytes.extend_from_slice(&chunk),
        Ok(Some(_)) => return Outcome::Retry("Registry response exceeds size limit", None),
        Ok(None) => break,
        Err(_) => return Outcome::Retry("Registry response read failed", None),
      }
    }
    let ack: Ack = match serde_json::from_slice(&bytes) {
      Ok(ack) => ack,
      Err(_) => return Outcome::Retry("invalid Registry acknowledgement", None),
    };
    if ack.node_id != self.node_id
      || ack.report_interval_seconds != 30
      || ack.expires_at <= ack.last_seen
      || ack.expires_at.saturating_sub(ack.last_seen) > 90
    {
      return Outcome::Retry("invalid Registry lease", None);
    }
    Outcome::Success
  }
}

#[derive(Debug, PartialEq)]
enum Outcome {
  Success,
  Retry(&'static str, Option<u64>),
  Stop(&'static str),
}
fn classify(status: StatusCode, retry_after: Option<&str>) -> Outcome {
  if status == StatusCode::TOO_MANY_REQUESTS {
    let delay = retry_after
      .and_then(|value| value.parse::<u64>().ok())
      .map(|value| value.clamp(1, 60));
    Outcome::Retry("Registry rate limited", delay)
  } else if status.is_server_error() {
    Outcome::Retry("Registry temporarily unavailable", None)
  } else if matches!(status.as_u16(), 401 | 403) {
    Outcome::Stop("Registry authentication or permission rejected")
  } else {
    Outcome::Stop("Registry rejected the report or endpoint; check configuration")
  }
}
fn jitter(seconds: u64) -> Duration {
  let mut random = [0];
  let _ = getrandom::fill(&mut random);
  Duration::from_millis(seconds * (800 + u64::from(random[0] % 41) * 10))
}
#[derive(Deserialize)]
struct Ack {
  node_id: String,
  last_seen: u64,
  expires_at: u64,
  report_interval_seconds: u64,
}
#[derive(Serialize)]
struct Report {
  version: &'static str,
  region: Option<String>,
  accepting_new_connections: bool,
  metrics: Metrics,
}
#[derive(Serialize)]
struct Metrics {
  quic_connections: usize,
  quic_connection_capacity: usize,
  websocket_connections: usize,
  websocket_connection_capacity: usize,
  live_bindings: usize,
  signalling_sessions: usize,
  signalling_session_capacity: Option<usize>,
}
fn snapshot(state: &State, region: Option<String>, accepting: bool) -> Report {
  let now = q2q_punch::binding::now();
  Report {
    version: env!("CARGO_PKG_VERSION"),
    region,
    accepting_new_connections: accepting,
    metrics: Metrics {
      quic_connections: CONNECTION_LIMIT.saturating_sub(state.quic_slots.available_permits()),
      quic_connection_capacity: CONNECTION_LIMIT,
      websocket_connections: CONNECTION_LIMIT
        .saturating_sub(state.websocket_slots.available_permits()),
      websocket_connection_capacity: CONNECTION_LIMIT,
      live_bindings: state.bindings.live_count(now),
      signalling_sessions: state.connections.live_count(now),
      signalling_session_capacity: None,
    },
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use q2q_primitives::identity::{PeerId, TicketBody};
  use q2q_punch::{
    binding::{Binding, Bindings},
    signal::Connections,
  };
  use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Semaphore,
  };

  fn state() -> Arc<State> {
    Arc::new(State {
      bindings: Bindings::new(300),
      connections: Connections::new(300),
      sinks: Arc::new(crate::signalling::Sinks::default()),
      limits: crate::limits::limiter(crate::limits::Rates::default(), 1),
      punch_id: "punch-1".into(),
      ttl: 300,
      max_request_bytes: 65536,
      quic_slots: Arc::new(Semaphore::new(CONNECTION_LIMIT)),
      websocket_slots: Arc::new(Semaphore::new(CONNECTION_LIMIT)),
    })
  }
  fn token(key: [u8; 32], node: &str) -> String {
    let claims = serde_json::json!({ "sub":node, "service_type":"punch", "public_key":STANDARD.encode(key),
      "advertise_url":"quic://punch.example:4433", "identity_version":1, "scope":["nodes:write:self"] });
    format!(
      "e30.{}.local-test-signature",
      URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap())
    )
  }
  fn config(url: String) -> Config {
    Config {
      url,
      token_file: None,
      region: Some("hk".into()),
      accepting_new_connections: true,
    }
  }

  #[test]
  fn endpoints_identity_and_secret_sources_are_checked() {
    for url in [
      "http://public.example",
      "https://user@host",
      "https://host/?secret=x",
      "https://host/#fragment",
    ] {
      assert!(endpoint(url).is_err());
    }
    assert_eq!(
      endpoint("http://[::1]:1234/base/").unwrap().as_str(),
      "http://[::1]:1234/base/v1/nodes/self"
    );
    let mut config = config("http://127.0.0.1:1234".into());
    assert!(Reporter::prepare(&config, "wrong", [1; 32], Some(token([1; 32], "punch-1"))).is_err());
    assert!(
      Reporter::prepare(&config, "punch-1", [2; 32], Some(token([1; 32], "punch-1"))).is_err()
    );
    config.token_file = Some("unused".into());
    assert!(
      Reporter::prepare(&config, "punch-1", [1; 32], Some(token([1; 32], "punch-1"))).is_err()
    );
  }

  #[test]
  fn snapshots_count_real_permits_and_ignore_unswept_expired_entries() {
    let state = state();
    let quic = state.quic_slots.clone().try_acquire_owned().unwrap();
    let ws = state
      .websocket_slots
      .clone()
      .try_acquire_many_owned(2)
      .unwrap();
    let now = q2q_punch::binding::now();
    for (peer, expiry) in [("live", now + 100), ("expired", now)] {
      state.bindings.insert(Binding {
        binding_id: peer.into(),
        peer_id: PeerId::unchecked(peer),
        key: [0; 32],
        expires_at: expiry,
        last_seq: 0,
        generation: 0,
      });
    }
    let ticket = TicketBody {
      v: 1,
      iss: "a".into(),
      public_key: STANDARD.encode([1; 32]),
      iat: now,
      exp: now + 300,
      aud: "punch:punch-1".into(),
      tgt: Some("b".into()),
      ih: None,
      cid: Some("session".into()),
      sig: String::new(),
    };
    state
      .connections
      .open(&ticket, &PeerId::unchecked("b"), true, now)
      .unwrap();
    assert_eq!(state.connections.live_count(now + 300), 0);
    let report = snapshot(&state, None, false);
    assert_eq!(report.metrics.quic_connections, 1);
    assert_eq!(report.metrics.websocket_connections, 2);
    assert_eq!(report.metrics.live_bindings, 1);
    assert_eq!(report.metrics.signalling_sessions, 1);
    assert_eq!(report.metrics.signalling_session_capacity, None);
    drop((quic, ws));
    let report = snapshot(&state, None, true);
    assert_eq!(report.metrics.quic_connections, 0);
    assert_eq!(report.metrics.websocket_connections, 0);
  }

  #[test]
  fn http_classification_has_bounded_backoff_and_terminal_auth_errors() {
    assert_eq!(
      classify(StatusCode::TOO_MANY_REQUESTS, Some("999999")),
      Outcome::Retry("Registry rate limited", Some(60))
    );
    assert_eq!(
      classify(StatusCode::TOO_MANY_REQUESTS, Some("0")),
      Outcome::Retry("Registry rate limited", Some(1))
    );
    assert!(matches!(
      classify(StatusCode::SERVICE_UNAVAILABLE, None),
      Outcome::Retry(_, None)
    ));
    for status in [401, 403, 409, 400, 302] {
      assert!(matches!(
        classify(StatusCode::from_u16(status).unwrap(), None),
        Outcome::Stop(_)
      ));
    }
    for _ in 0..100 {
      let delay = jitter(30);
      assert!((Duration::from_secs(24)..=Duration::from_secs(36)).contains(&delay));
    }
  }

  async fn mock_server(
    body: &'static str,
    status: u16,
  ) -> (String, tokio::task::JoinHandle<serde_json::Value>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
      let (mut stream, _) = listener.accept().await.unwrap();
      let mut bytes = Vec::new();
      loop {
        let mut buffer = [0; 2048];
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(read > 0);
        bytes.extend_from_slice(&buffer[..read]);
        if let Some(split) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
          let headers = String::from_utf8_lossy(&bytes[..split]);
          let length: usize = headers
            .lines()
            .find_map(|line| {
              line
                .to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|s| s.trim().parse().unwrap())
            })
            .unwrap();
          if bytes.len() >= split + 4 + length {
            let report = serde_json::from_slice(&bytes[split + 4..split + 4 + length]).unwrap();
            let reply = format!(
              "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
              body.len(),
              body
            );
            stream.write_all(reply.as_bytes()).await.unwrap();
            return report;
          }
        }
      }
    });
    (url, task)
  }

  #[tokio::test]
  async fn real_http_report_requires_matching_bounded_ack_and_sends_latest_metrics() {
    let (url, task) = mock_server(
      r#"{"node_id":"punch-1","last_seen":100,"expires_at":190,"report_interval_seconds":30}"#,
      200,
    )
    .await;
    let reporter = Reporter::prepare(
      &config(url),
      "punch-1",
      [1; 32],
      Some(token([1; 32], "punch-1")),
    )
    .unwrap();
    assert_eq!(
      reporter
        .send(snapshot(&state(), Some("hk".into()), true))
        .await,
      Outcome::Success
    );
    let report = task.await.unwrap();
    assert_eq!(report["metrics"]["quic_connection_capacity"], 1024);
    assert!(report["metrics"]["signalling_session_capacity"].is_null());
    assert_eq!(report["region"], "hk");
    let (url, task) = mock_server(
      r#"{"node_id":"other","last_seen":100,"expires_at":190,"report_interval_seconds":30}"#,
      200,
    )
    .await;
    let reporter = Reporter::prepare(
      &config(url),
      "punch-1",
      [1; 32],
      Some(token([1; 32], "punch-1")),
    )
    .unwrap();
    assert!(matches!(
      reporter.send(snapshot(&state(), None, true)).await,
      Outcome::Retry(_, None)
    ));
    task.await.unwrap();
  }

  #[tokio::test]
  async fn shutdown_cancels_an_inflight_report_without_waiting_for_request_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = config(format!("http://{}", listener.local_addr().unwrap()));
    let reporter =
      Reporter::prepare(&config, "punch-1", [1; 32], Some(token([1; 32], "punch-1"))).unwrap();
    let (stop, rx) = watch::channel(false);
    let task = tokio::spawn(reporter.run(state(), rx));
    let (_stalled, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
      .await
      .unwrap()
      .unwrap();
    stop.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(1), task)
      .await
      .unwrap()
      .unwrap();
  }
  #[tokio::test]
  async fn auth_rejection_stops_only_the_reporting_task() {
    let (url, request) = mock_server("{}", 401).await;
    let reporter = Reporter::prepare(
      &config(url),
      "punch-1",
      [1; 32],
      Some(token([1; 32], "punch-1")),
    )
    .unwrap();
    let state = state();
    let (_stop, rx) = watch::channel(false);
    let task = tokio::spawn(reporter.run(state.clone(), rx));
    request.await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), task)
      .await
      .unwrap()
      .unwrap();
    assert!(state.quic_slots.clone().try_acquire_owned().is_ok());
    assert!(state.websocket_slots.clone().try_acquire_owned().is_ok());
  }
}
