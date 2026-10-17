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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock as SyncRwLock};
use std::time::Duration;
use tokio::sync::{Mutex, RwLock, watch};
use url::Url;

const SESSION_TTL: u64 = 300;

struct TrackAuth {
  key: [u8; 32],
  seq: u64,
}

/// A shared control-plane presence with serialized Track session rotations.
pub struct ControlSession {
  online: crate::online::Online,
  http: reqwest::Client,
  identity: Identity,
  track_url: Url,
  peer_id: PeerId,
  track_auth: Mutex<TrackAuth>,
  track_expires_at: AtomicU64,
  heartbeat_interval: u32,
  turn: RwLock<q2q_track::wire::TurnWire>,
  assignment: SyncRwLock<q2q_track::wire::PunchWire>,
  punch: RwLock<Arc<PunchBinding>>,
  punch_changed: watch::Sender<u64>,
  upload_admission: AtomicBool,
  admission: Arc<crate::admission::Admission>,
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
      online: crate::online::Online::default(),
      http,
      identity: identity.clone(),
      track_url: track_url.clone(),
      peer_id,
      track_auth: Mutex::new(TrackAuth {
        key: session_key,
        seq: 0,
      }),
      track_expires_at: AtomicU64::new(announced.expires_at),
      heartbeat_interval: announced.heartbeat_interval,
      turn: RwLock::new(announced.turn),
      assignment: SyncRwLock::new(announced.punch),
      punch: RwLock::new(Arc::new(punch)),
      punch_changed: watch::channel(0).0,
      upload_admission: AtomicBool::new(true),
      admission: Arc::new(crate::admission::Admission::default()),
    })
  }

  /// Active publications/downloads share a heartbeat task. The last user
  /// dropping its lease aborts the task without retaining the session forever.
  pub(crate) async fn keep_online(self: &Arc<Self>) -> ControlError {
    let session = self.clone();
    let lease = self.online.lease(async move {
      loop {
        tokio::time::sleep(Duration::from_secs(
          u64::from(session.heartbeat_interval()).max(1),
        ))
        .await;
        if let Err(error) = session.heartbeat().await {
          return error;
        }
      }
    });
    ControlError::Shared(lease.failed().await)
  }

  pub fn peer_id(&self) -> &PeerId {
    &self.peer_id
  }

  pub fn punch_id(&self) -> String {
    self
      .assignment
      .read()
      .unwrap_or_else(|e| e.into_inner())
      .punch_id
      .clone()
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
    let turn = self.turn.read().await.clone();
    peer::dial(&self.identity, target, info_hash, &turn).await
  }

  /// Accept the next request for a specific content hash on this Punch Binding.
  /// Concurrent caller-managed accepts are allowed. A managed publication on
  /// this session owns admission exclusively and returns AdmissionConflict.
  pub async fn accept_peer(&self, info_hash: [u8; 32]) -> Result<PeerSession, ControlError> {
    // Keep the lease through negotiation. A managed publisher must not pause
    // this reader, and this API must not reopen a publisher's closed gate.
    let _lease = self.admission.manual()?;
    self.set_upload_admission(true).await;
    self.incoming_peer(info_hash).await?.accept().await
  }

  pub(crate) fn claim_publication(&self) -> Result<crate::admission::AdmissionLease, ControlError> {
    self.admission.managed()
  }

  pub(crate) async fn set_upload_admission(&self, enabled: bool) {
    let punch = self.punch.read().await;
    self.upload_admission.store(enabled, Ordering::Release);
    punch.set_accepting(enabled).await;
  }

  pub(crate) async fn incoming_peer(
    &self,
    info_hash: [u8; 32],
  ) -> Result<peer::PendingPeer, ControlError> {
    let mut changed = self.punch_changed.subscribe();
    loop {
      let punch = self.punch.read().await.clone();
      let turn = self.turn.read().await.clone();
      tokio::select! {
        request = peer::incoming(&self.identity, punch, info_hash, turn) => return request,
        _ = changed.changed() => {},
      }
    }
  }

  /// Refresh both sessions. Sequence numbers are consumed before I/O so a
  /// reply lost after server acceptance cannot cause a replay on retry.
  pub async fn heartbeat(&self) -> Result<(), ControlError> {
    let mut auth = self.track_auth.lock().await;
    let seq = next_seq(&mut auth.seq)?;
    let now = q2q_track::state::now();
    let request = HeartbeatRequest {
      seq,
      timestamp: now,
      mac: B64.encode(mac_bytes(&auth.key, &self.peer_id, seq, now)),
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
    drop(auth);
    self.punch.read().await.clone().heartbeat().await
  }

  /// Query the current Track index. The hash must be the 32 raw bytes of an
  /// info hash, not a caller-chosen filename.
  pub async fn query(&self, info_hash: &[u8; 32]) -> Result<QueryResponse, ControlError> {
    let mut auth = self.track_auth.lock().await;
    let seq = next_seq(&mut auth.seq)?;
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
        &B64.encode(mac_bytes(&auth.key, &self.peer_id, seq, now)),
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
  pub async fn close(&self) -> Result<(), ControlError> {
    self.punch.read().await.clone().close().await
  }

  /// Update complete holdings. Track rotates its session key on every announce;
  /// serialize the rotation with query/heartbeat and adopt the new key first.
  pub(crate) async fn update_files(
    self: &Arc<Self>,
    add: &[[u8; 32]],
    del: &[[u8; 32]],
  ) -> Result<u64, ControlError> {
    let session = self.clone();
    let add = add.to_vec();
    let del = del.to_vec();
    // Once submitted, a rotating announce must adopt its returned key even if
    // its caller is cancelled. The next query/withdrawal waits for this task's
    // auth lock rather than racing a server-side rotation it did not observe.
    tokio::spawn(async move { session.update_files_inner(&add, &del).await })
      .await
      .map_err(|e| ControlError::Protocol(format!("announcement task failed: {e}")))?
  }

  async fn update_files_inner(
    &self,
    add: &[[u8; 32]],
    del: &[[u8; 32]],
  ) -> Result<u64, ControlError> {
    let mut auth = self.track_auth.lock().await;
    let hashes = |list: &[[u8; 32]]| {
      list
        .iter()
        .map(|hash| format!("sha256:{}", hex(hash)))
        .collect::<Vec<_>>()
    };
    let add = hashes(add);
    let request = AnnounceRequest {
      token: self
        .identity
        .ticket(Audience::Track, q2q_track::state::now(), None),
      complete: add.clone(),
      add,
      del: hashes(del),
      ttl: SESSION_TTL,
    };
    let url = endpoint(&self.track_url, &["peers", self.peer_id.as_str(), "files"])?;
    let response = self
      .http
      .put(url)
      .json(&request)
      .send()
      .await
      .map_err(ControlError::Http)?;
    let announced: AnnounceResponse = expect_success(response, "Track")
      .await?
      .json()
      .await
      .map_err(ControlError::Http)?;
    auth.key = decode_key(&announced.session_key, "Track session_key")?;
    auth.seq = 0;
    self
      .track_expires_at
      .store(announced.expires_at, Ordering::Release);
    let changed = {
      let current = self.assignment.read().unwrap_or_else(|e| e.into_inner());
      current.punch_id != announced.punch.punch_id
        || current.url != announced.punch.url
        || current.public_key != announced.punch.public_key
    };
    if changed {
      let replacement = Arc::new(PunchBinding::connect(&self.identity, &announced.punch).await?);
      let previous = {
        let mut punch = self.punch.write().await;
        replacement
          .set_accepting(self.upload_admission.load(Ordering::Acquire))
          .await;
        std::mem::replace(&mut *punch, replacement)
      };
      *self.assignment.write().unwrap_or_else(|e| e.into_inner()) = announced.punch;
      self
        .punch_changed
        .send_modify(|generation| *generation = generation.wrapping_add(1));
      let _ = previous.close().await;
    }
    *self.turn.write().await = announced.turn;
    Ok(announced.expires_at)
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
  /// A managed publication and caller-managed accept cannot own admission together.
  AdmissionConflict,
  /// The standing channel stopped admitting new connections.
  AdmissionPaused,
  /// The remote publisher has no upload connection slot available.
  Busy,
  /// A shared session heartbeat failure; the original typed error is retained.
  Shared(Arc<ControlError>),
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
      Self::AdmissionConflict => f.write_str("peer admission is owned by another operation"),
      Self::AdmissionPaused => f.write_str("peer admission is paused"),
      Self::Busy => f.write_str("peer upload capacity is full"),
      Self::Shared(e) => e.fmt(f),
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
      Self::Shared(e) => Some(e.as_ref()),
      Self::Http(e) => Some(e),
      Self::Random(e) => Some(e),
      _ => None,
    }
  }
}
