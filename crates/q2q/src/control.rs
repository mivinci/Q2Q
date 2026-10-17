//! The authenticated Track and Punch control-plane session.

use crate::identity::Identity;
use crate::peer::{self, PeerSession};
use crate::punch_transport::PunchBinding;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use q2q_core::identity::{Audience, PeerId};
use q2q_track::auth::mac_bytes;
use q2q_track::wire::{
  AnnounceRequest, AnnounceResponse, ErrorBody, HeartbeatRequest, HeartbeatResponse, QueryResponse,
};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::Mutex;
use url::Url;

const SESSION_TTL: u64 = 300;

/// An online control-plane presence. File announcements and WebRTC transfer
/// will be attached to this session in the next step.
pub struct ControlSession {
  http: reqwest::Client,
  identity: Identity,
  track_url: Url,
  peer_id: PeerId,
  session_key: [u8; 32],
  track_seq: Mutex<u64>,
  track_expires_at: AtomicU64,
  heartbeat_interval: u32,
  turn: q2q_track::wire::TurnWire,
  punch: PunchBinding,
}

impl ControlSession {
  pub(super) async fn connect(identity: &Identity, track_url: &Url) -> Result<Self, ControlError> {
    let http = reqwest::Client::builder()
      .timeout(Duration::from_secs(10))
      .build()
      .map_err(ControlError::Http)?;
    let peer_id = identity.peer_id().clone();
    let url = endpoint(track_url, &["peers", peer_id.as_str(), "files"])?;
    let now = q2q_track::state::now();
    let request = AnnounceRequest {
      token: identity.ticket(Audience::Track, now, None),
      add: Vec::new(),
      del: Vec::new(),
      complete: Vec::new(),
      ttl: SESSION_TTL,
    };
    let response = http
      .put(url)
      .json(&request)
      .send()
      .await
      .map_err(ControlError::Http)?;
    let response = expect_success(response, "Track").await?;
    let announced: AnnounceResponse = response.json().await.map_err(ControlError::Http)?;
    let session_key = decode_key(&announced.session_key, "Track session_key")?;
    let punch = PunchBinding::connect(identity, &announced.punch).await?;
    Ok(Self {
      http,
      identity: identity.clone(),
      track_url: track_url.clone(),
      peer_id,
      session_key,
      track_seq: Mutex::new(0),
      track_expires_at: AtomicU64::new(announced.expires_at),
      heartbeat_interval: announced.heartbeat_interval,
      turn: announced.turn,
      punch,
    })
  }

  pub fn peer_id(&self) -> &PeerId {
    &self.peer_id
  }

  pub fn punch_id(&self) -> &str {
    self.punch.punch_id()
  }

  pub fn heartbeat_interval(&self) -> u32 {
    self.heartbeat_interval
  }

  pub fn track_expires_at(&self) -> u64 {
    self.track_expires_at.load(Ordering::Acquire)
  }

  /// Connect to a queried peer through that peer's assigned Punch.
  /// Returns after mutual identity confirmation on the WebRTC channel.
  pub async fn dial_peer(
    &self,
    target: &q2q_track::wire::PeerWire,
    info_hash: [u8; 32],
  ) -> Result<PeerSession, ControlError> {
    peer::dial(&self.identity, target, info_hash, &self.turn).await
  }

  /// Accept the next request for a specific content hash on this Punch Binding.
  pub async fn accept_peer(&self, info_hash: [u8; 32]) -> Result<PeerSession, ControlError> {
    peer::accept(&self.identity, &self.punch, info_hash, &self.turn).await
  }

  /// Refresh both sessions. Sequence numbers are consumed before I/O so a
  /// reply lost after server acceptance cannot cause a replay on retry.
  pub async fn heartbeat(&self) -> Result<(), ControlError> {
    let mut track_seq = self.track_seq.lock().await;
    let seq = next_seq(&mut track_seq)?;
    let now = q2q_track::state::now();
    let request = HeartbeatRequest {
      seq,
      timestamp: now,
      mac: B64.encode(mac_bytes(&self.session_key, &self.peer_id, seq, now)),
    };
    let url = endpoint(
      &self.track_url,
      &["peers", self.peer_id.as_str(), "heartbeat"],
    )?;
    let response = self
      .http
      .post(url)
      .json(&request)
      .send()
      .await
      .map_err(ControlError::Http)?;
    let response = expect_success(response, "Track").await?;
    let heartbeat: HeartbeatResponse = response.json().await.map_err(ControlError::Http)?;
    self
      .track_expires_at
      .store(heartbeat.expires_at, Ordering::Release);
    drop(track_seq);
    self.punch.heartbeat().await
  }

  /// Query the current Track index. The hash must be the 32 raw bytes of an
  /// info hash, not a caller-chosen filename.
  pub async fn query(&self, info_hash: &[u8; 32]) -> Result<QueryResponse, ControlError> {
    let mut track_seq = self.track_seq.lock().await;
    let seq = next_seq(&mut track_seq)?;
    let now = q2q_track::state::now();
    let hash = format!("sha256:{}", hex(info_hash));
    let mut url = endpoint(&self.track_url, &["files", &hash, "peers"])?;
    url
      .query_pairs_mut()
      .append_pair("peer_id", self.peer_id.as_str())
      .append_pair("seq", &seq.to_string())
      .append_pair("timestamp", &now.to_string())
      .append_pair(
        "mac",
        &B64.encode(mac_bytes(&self.session_key, &self.peer_id, seq, now)),
      );
    let response = self
      .http
      .get(url)
      .send()
      .await
      .map_err(ControlError::Http)?;
    let response = expect_success(response, "Track").await?;
    response.json().await.map_err(ControlError::Http)
  }

  /// Leave the Punch Binding and close the authenticated QUIC channel.
  pub async fn close(mut self) -> Result<(), ControlError> {
    self.punch.close().await
  }
}

fn endpoint(base: &Url, segments: &[&str]) -> Result<Url, ControlError> {
  let mut url = base.clone();
  url.set_query(None);
  url.set_fragment(None);
  url
    .path_segments_mut()
    .map_err(|_| ControlError::Protocol("Track URL cannot carry path segments".into()))?
    .pop_if_empty()
    .extend(segments);
  Ok(url)
}

pub(crate) fn decode_key(encoded: &str, field: &str) -> Result<[u8; 32], ControlError> {
  B64
    .decode(encoded)
    .map_err(|_| ControlError::Protocol(format!("{field} is not base64")))?
    .try_into()
    .map_err(|_| ControlError::Protocol(format!("{field} must be 32 bytes")))
}

pub(crate) fn next_seq(last: &mut u64) -> Result<u64, ControlError> {
  *last = last
    .checked_add(1)
    .ok_or_else(|| ControlError::Protocol("session sequence exhausted".into()))?;
  Ok(*last)
}

pub(crate) fn hex(bytes: &[u8]) -> String {
  bytes.iter().map(|b| format!("{b:02x}")).collect()
}

async fn expect_success(
  response: reqwest::Response,
  service: &'static str,
) -> Result<reqwest::Response, ControlError> {
  if response.status().is_success() {
    return Ok(response);
  }
  let status = response.status().as_u16();
  let error = response.json::<ErrorBody>().await.ok();
  Err(ControlError::Remote {
    service,
    status,
    code: error.as_ref().map(|e| e.code.clone()),
    detail: error.and_then(|e| e.detail),
  })
}

#[derive(Debug)]
pub enum ControlError {
  Http(reqwest::Error),
  Remote {
    service: &'static str,
    status: u16,
    code: Option<String>,
    detail: Option<String>,
  },
  Transport(String),
  Authentication(String),
  Protocol(String),
  Random(getrandom::Error),
}

impl fmt::Display for ControlError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::Http(e) => write!(f, "Track request failed: {e}"),
      Self::Remote {
        service,
        status,
        code,
        detail,
      } => {
        write!(f, "{service} returned {status}")?;
        if let Some(code) = code {
          write!(f, " ({code})")?;
        }
        if let Some(detail) = detail {
          write!(f, ": {detail}")?;
        }
        Ok(())
      }
      Self::Transport(e) => write!(f, "Punch transport failed: {e}"),
      Self::Authentication(e) => write!(f, "peer identity confirmation failed: {e}"),
      Self::Protocol(e) => write!(f, "Q2Q protocol error: {e}"),
      Self::Random(e) => write!(f, "could not generate a request id: {e}"),
    }
  }
}

impl std::error::Error for ControlError {
  fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
    match self {
      Self::Http(e) => Some(e),
      Self::Random(e) => Some(e),
      _ => None,
    }
  }
}
