//! The three HTTP endpoints, plus TURN credential minting.
//!
//! Track never issues identity tickets — clients sign their own. The one thing
//! it *does* issue is a TURN credential, because relay capacity costs money and
//! has to be rate limited somewhere.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use hmac::{Hmac, KeyInit, Mac};
use q2q_core::identity::PeerId;
use q2q_track::auth::{self, AuthError, TicketBody};
use q2q_track::state::{Candidate, InfoHash, PunchAssignment, TrackState, UnixSeconds};
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use std::sync::Arc;

pub const DEFAULT_LIMIT: usize = 10;
pub const MAX_LIMIT: usize = 50;
pub const DEFAULT_HEARTBEAT_INTERVAL: u32 = 30;

/// Where Track gets its Punch list and TURN secret from.
#[derive(Debug, Clone)]
pub struct TrackConfig {
  pub punch_servers: Vec<PunchAssignment>,
  pub turn_url: String,
  /// Shared secret for coturn's REST API mechanism.
  pub turn_secret: String,
  /// Credentials outlive a single ticket on purpose; see spec 3.4.1.
  pub turn_ttl: UnixSeconds,
  pub max_ttl: UnixSeconds,
  pub heartbeat_interval: u32,
}

impl Default for TrackConfig {
  fn default() -> Self {
    TrackConfig {
      punch_servers: Vec::new(),
      turn_url: "turn:turn.example.com:3478".into(),
      turn_secret: String::new(),
      turn_ttl: 3600,
      max_ttl: 3600,
      heartbeat_interval: DEFAULT_HEARTBEAT_INTERVAL,
    }
  }
}

pub struct AppState<S: TrackState> {
  pub state: Arc<S>,
  pub config: Arc<TrackConfig>,
}

// Hand-written: deriving Clone would require `S: Clone`, which state backends
// are not (and need not be — they are shared behind an Arc).
impl<S: TrackState> Clone for AppState<S> {
  fn clone(&self) -> Self {
    AppState {
      state: self.state.clone(),
      config: self.config.clone(),
    }
  }
}

pub fn router<S: TrackState + 'static>(state: Arc<S>, config: Arc<TrackConfig>) -> Router {
  let app = AppState { state, config };
  Router::new()
    .route("/peers/{peer_id}/resources", put(announce::<S>))
    .route("/peers/{peer_id}/heartbeat", post(heartbeat::<S>))
    .route("/resources/{info_hash}/peers", get(query::<S>))
    .with_state(app)
}

/// A uniform error body: a code the client can branch on, plus a request id.
#[derive(Debug, Clone, Serialize)]
pub struct ErrorBody {
  pub code: &'static str,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub detail: Option<String>,
}

fn fail(code: &'static str, detail: Option<String>) -> (StatusCode, Json<ErrorBody>) {
  let status = match code {
    "malformed_ticket" | "bad_request" => StatusCode::BAD_REQUEST,
    "bad_signature" | "key_not_issuer" | "expired" | "wrong_audience" | "bad_mac"
    | "stale_timestamp" => StatusCode::UNAUTHORIZED,
    "unknown_session" => StatusCode::UNAUTHORIZED,
    _ => StatusCode::INTERNAL_SERVER_ERROR,
  };

  // one place decides how loud a rejection is: a malformed request from a
  // client is routine, a failed signature is someone probing, a 5xx is ours
  match status {
    StatusCode::INTERNAL_SERVER_ERROR => tracing::error!(code, "request failed"),
    StatusCode::UNAUTHORIZED => tracing::warn!(code, "request rejected"),
    _ => tracing::debug!(code, ?detail, "request rejected"),
  }
  (status, Json(ErrorBody { code, detail }))
}

fn auth_failure(e: AuthError) -> (StatusCode, Json<ErrorBody>) {
  let detail = match &e {
    AuthError::MalformedTicket(d) => Some(d.clone()),
    _ => None,
  };
  // fail() logs it; auth rejections are the interesting kind
  fail(e.code(), detail)
}

// ------------------------------------------------------------------ announce

#[derive(Debug, Deserialize)]
pub struct AnnounceRequest {
  pub token: TicketBody,
  #[serde(default)]
  pub add: Vec<String>,
  #[serde(default)]
  pub del: Vec<String>,
  #[serde(default)]
  pub complete: Vec<String>,
  pub ttl: UnixSeconds,
}

#[derive(Debug, Serialize)]
pub struct AnnounceResponse {
  pub session_key: String,
  pub punch: PunchWire,
  pub turn: TurnWire,
  pub heartbeat_interval: u32,
  pub expires_at: UnixSeconds,
}

#[derive(Debug, Serialize)]
pub struct PunchWire {
  pub punch_id: String,
  pub url: String,
  pub public_key: String,
}

#[derive(Debug, Serialize)]
pub struct TurnWire {
  pub url: String,
  pub username: String,
  pub credential: String,
  pub expires_at: UnixSeconds,
}

/// Parse `sha256:<hex>` into the raw 32 bytes.
fn parse_info_hash(s: &str) -> Option<InfoHash> {
  let hex = s.strip_prefix("sha256:")?;
  if hex.len() != 64 {
    return None;
  }
  let mut out = [0u8; 32];
  for (i, chunk) in out.iter_mut().enumerate() {
    *chunk = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
  }
  Some(out)
}

async fn announce<S: TrackState>(
  State(app): State<AppState<S>>,
  Path(peer_id): Path<String>,
  Json(req): Json<AnnounceRequest>,
) -> Result<Json<AnnounceResponse>, (StatusCode, Json<ErrorBody>)> {
  let now = q2q_track::state::now();

  if peer_id != req.token.iss {
    return Err(fail(
      "bad_request",
      Some("path peer_id does not match ticket iss".into()),
    ));
  }
  let ticket = auth::verify_announce(&req.token, now).map_err(auth_failure)?;

  let add: Vec<InfoHash> = req
    .add
    .iter()
    .map(|s| {
      parse_info_hash(s).ok_or_else(|| fail("bad_request", Some(format!("bad info_hash: {s}"))))
    })
    .collect::<Result<_, _>>()?;
  let del: Vec<InfoHash> = req
    .del
    .iter()
    .map(|s| {
      parse_info_hash(s).ok_or_else(|| fail("bad_request", Some(format!("bad info_hash: {s}"))))
    })
    .collect::<Result<_, _>>()?;
  let complete: Vec<InfoHash> = req
    .complete
    .iter()
    .map(|s| {
      parse_info_hash(s).ok_or_else(|| fail("bad_request", Some(format!("bad info_hash: {s}"))))
    })
    .collect::<Result<_, _>>()?;

  // server caps the ttl, as spec 3.4 requires
  let ttl = req.ttl.min(app.config.max_ttl);
  let expires_at = now + ttl;

  let punch = app
    .config
    .punch_servers
    .first()
    .cloned()
    .unwrap_or(PunchAssignment {
      punch_id: "none".into(),
      url: String::new(),
      public_key: Vec::new(),
    });

  let session_key = random_32();
  app.state.put_online(q2q_track::state::OnlineRecord {
    peer_id: ticket.iss.clone(),
    public_key: ticket.public_key,
    session_key,
    last_seq: 0,
    punch: punch.clone(),
    expires_at,
  });
  app
    .state
    .update_resources(&ticket.iss, &add, &del, &complete, ttl);

  let turn_expires_at = now + app.config.turn_ttl;
  tracing::info!(
    peer_id = %ticket.iss.as_str(),
    add = add.len(),
    del = del.len(),
    "announce"
  );
  Ok(Json(AnnounceResponse {
    session_key: B64.encode(session_key),
    punch: PunchWire {
      punch_id: punch.punch_id,
      url: punch.url,
      public_key: B64.encode(punch.public_key),
    },
    turn: turn_credential(&app.config, &peer_id, turn_expires_at),
    heartbeat_interval: app.config.heartbeat_interval,
    expires_at,
  }))
}

/// coturn's REST API mechanism: `username = "<expiry>:<user>"`, credential is
/// HMAC-SHA1 of that username under the shared secret.
fn turn_credential(config: &TrackConfig, user: &str, expires_at: UnixSeconds) -> TurnWire {
  let username = format!("{expires_at}:{user}");
  let credential = Hmac::<Sha1>::new_from_slice(config.turn_secret.as_bytes())
    .expect("HMAC accepts any key length")
    .chain_update(username.as_bytes())
    .finalize()
    .into_bytes();
  TurnWire {
    url: config.turn_url.clone(),
    username,
    credential: B64.encode(credential),
    expires_at,
  }
}

fn random_32() -> [u8; 32] {
  let mut out = [0u8; 32];
  getrandom::fill(&mut out).expect("system CSPRNG");
  out
}

// ----------------------------------------------------------------- heartbeat

#[derive(Debug, Deserialize)]
pub struct HeartbeatRequest {
  pub seq: u64,
  pub timestamp: UnixSeconds,
  pub mac: String,
}

#[derive(Debug, Serialize)]
pub struct HeartbeatResponse {
  pub expires_at: UnixSeconds,
}

async fn heartbeat<S: TrackState>(
  State(app): State<AppState<S>>,
  Path(peer_id): Path<String>,
  Json(req): Json<HeartbeatRequest>,
) -> Result<Json<HeartbeatResponse>, (StatusCode, Json<ErrorBody>)> {
  let now = q2q_track::state::now();
  let peer = PeerId::unchecked(peer_id);

  let record = app
    .state
    .online(&peer)
    .ok_or_else(|| fail("unknown_session", None))?;

  auth::verify_session_request(
    &record.session_key,
    &peer,
    req.seq,
    req.timestamp,
    &req.mac,
    record.last_seq,
    now,
  )
  .map_err(auth_failure)?;

  // the strict replay check: auth only absorbs reordering
  app
    .state
    .touch_sequence(&peer, req.seq)
    .map_err(|_| fail("bad_mac", None))?;

  let ttl = app.config.max_ttl.min(3600);
  let expires_at = now + ttl;
  app.state.put_online(q2q_track::state::OnlineRecord {
    expires_at,
    ..record
  });

  Ok(Json(HeartbeatResponse { expires_at }))
}

// --------------------------------------------------------------------- query

#[derive(Debug, Deserialize)]
pub struct QueryParams {
  #[serde(default)]
  pub limit: Option<usize>,
  #[serde(default)]
  pub cursor: Option<String>,
  /// Session auth: query carries no body, so these travel as parameters.
  pub peer_id: Option<String>,
  pub seq: Option<u64>,
  pub timestamp: Option<UnixSeconds>,
  pub mac: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct QueryResponse {
  pub peers: Vec<PeerWire>,
  pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PeerWire {
  pub peer_id: String,
  pub public_key: String,
  pub punch_server: String,
  pub complete: bool,
}

async fn query<S: TrackState>(
  State(app): State<AppState<S>>,
  Path(info_hash): Path<String>,
  Query(params): Query<QueryParams>,
) -> Result<Json<QueryResponse>, (StatusCode, Json<ErrorBody>)> {
  let now = q2q_track::state::now();

  // spec 7.2.1 says query carries the session; there is no body, so the
  // HMAC triple rides along as query parameters.
  if let (Some(pid), Some(seq), Some(ts), Some(mac)) =
    (&params.peer_id, params.seq, params.timestamp, &params.mac)
  {
    let peer = PeerId::unchecked(pid.clone());
    let record = app
      .state
      .online(&peer)
      .ok_or_else(|| fail("unknown_session", None))?;
    auth::verify_session_request(
      &record.session_key,
      &peer,
      seq,
      ts,
      mac,
      record.last_seq,
      now,
    )
    .map_err(auth_failure)?;
    app
      .state
      .touch_sequence(&peer, seq)
      .map_err(|_| fail("bad_mac", None))?;
  }

  let hash = parse_info_hash(&info_hash)
    .ok_or_else(|| fail("bad_request", Some("info_hash must be sha256:<hex>".into())))?;
  let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
  let after = params.cursor.as_ref().map(|c| PeerId::unchecked(c.clone()));

  let peers: Vec<PeerWire> = app
    .state
    .query(&hash, limit + 1, after.as_ref(), now)
    .into_iter()
    .map(|c: Candidate| PeerWire {
      peer_id: c.peer_id.as_str().to_string(),
      public_key: B64.encode(c.public_key),
      punch_server: c.punch_server,
      complete: c.complete,
    })
    .collect();

  // one extra was fetched only to learn whether another page exists
  let has_more = peers.len() > limit;
  let peers: Vec<PeerWire> = peers.into_iter().take(limit).collect();
  let next_cursor = if has_more {
    peers.last().map(|p| p.peer_id.clone())
  } else {
    None
  };

  Ok(Json(QueryResponse { peers, next_cursor }))
}

#[cfg(test)]
mod tests {
  use super::*;
  use axum::body::Body;
  use axum::http::{Request, StatusCode};
  use q2q_core::identity::{Audience, Ticket, keypair_from_seed, peer_id_of};
  use q2q_track::state::MemoryState;
  use tower::ServiceExt;

  fn config() -> Arc<TrackConfig> {
    Arc::new(TrackConfig {
      punch_servers: vec![PunchAssignment {
        punch_id: "punch-1".into(),
        url: "https://punch-1.example.com".into(),
        public_key: vec![7u8; 32],
      }],
      turn_url: "turn:turn.example.com:3478".into(),
      turn_secret: "test-secret".into(),
      turn_ttl: 3600,
      max_ttl: 600,
      heartbeat_interval: 30,
    })
  }

  fn ticket_body(seed: u8, aud: &str) -> TicketBody {
    let (sk, _) = keypair_from_seed(&[seed; 32]);
    let mut t = Ticket {
      v: 1,
      iss: peer_id_of(&sk),
      public_key: sk.verifying_key().to_bytes(),
      iat: q2q_track::state::now(),
      exp: q2q_track::state::now() + 600,
      aud: Audience::from_wire(aud),
      tgt: None,
      ih: None,
      cid: None,
      sig: [0u8; 64],
    };
    t.sign(&sk);
    TicketBody {
      v: t.v,
      iss: t.iss.as_str().to_string(),
      public_key: B64.encode(t.public_key),
      iat: t.iat,
      exp: t.exp,
      aud: aud.to_string(),
      tgt: None,
      ih: None,
      cid: None,
      sig: B64.encode(t.sig),
    }
  }

  async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
      .await
      .unwrap();
    serde_json::from_slice(&bytes).unwrap()
  }

  #[tokio::test]
  async fn announce_establishes_a_session() {
    let app = router(Arc::new(MemoryState::new()), config());
    let body = ticket_body(1, "track");
    let peer = body.iss.clone();
    let req = Request::builder()
      .method("PUT")
      .uri(format!("/peers/{peer}/resources"))
      .header("content-type", "application/json")
      .body(Body::from(
        serde_json::to_vec(&serde_json::json!({
          "token": body, "add": ["sha256:".to_owned() + &"ab".repeat(32)], "ttl": 600
        }))
        .unwrap(),
      ))
      .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert!(json["session_key"].is_string());
    assert_eq!(json["punch"]["punch_id"], "punch-1");
    assert_eq!(json["heartbeat_interval"], 30);
  }

  #[tokio::test]
  async fn announce_rejects_a_ticket_for_another_audience() {
    let app = router(Arc::new(MemoryState::new()), config());
    let body = ticket_body(2, "punch-9");
    let peer = body.iss.clone();
    let req = Request::builder()
      .method("PUT")
      .uri(format!("/peers/{peer}/resources"))
      .header("content-type", "application/json")
      .body(Body::from(
        serde_json::to_vec(&serde_json::json!({"token": body, "ttl": 600})).unwrap(),
      ))
      .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(body_json(resp).await["code"], "wrong_audience");
  }

  #[tokio::test]
  async fn announce_caps_ttl() {
    let app = router(Arc::new(MemoryState::new()), config());
    let body = ticket_body(3, "track");
    let peer = body.iss.clone();
    let req = Request::builder()
      .method("PUT")
      .uri(format!("/peers/{peer}/resources"))
      .header("content-type", "application/json")
      .body(Body::from(
        serde_json::to_vec(&serde_json::json!({"token": body, "ttl": 999_999})).unwrap(),
      ))
      .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let json = body_json(resp).await;
    // max_ttl is 600, so expiry must be well short of what was asked for
    let expires_at = json["expires_at"].as_u64().unwrap();
    assert!(expires_at < q2q_track::state::now() + 601);
  }

  #[tokio::test]
  async fn heartbeat_without_a_session_is_rejected() {
    let app = router(Arc::new(MemoryState::new()), config());
    let req = Request::builder()
      .method("POST")
      .uri("/peers/whatever/heartbeat")
      .header("content-type", "application/json")
      .body(Body::from(
        serde_json::to_vec(
          &serde_json::json!({"seq": 1, "timestamp": q2q_track::state::now(), "mac": ""}),
        )
        .unwrap(),
      ))
      .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(body_json(resp).await["code"], "unknown_session");
  }

  #[tokio::test]
  async fn query_returns_the_holder() {
    let state = Arc::new(MemoryState::new());
    let app = router(state.clone(), config());
    let body = ticket_body(4, "track");
    let peer = body.iss.clone();
    let hash = "ab".repeat(32);

    let announce_req = Request::builder()
      .method("PUT")
      .uri(format!("/peers/{peer}/resources"))
      .header("content-type", "application/json")
      .body(Body::from(
        serde_json::to_vec(&serde_json::json!({
          "token": body, "add": [format!("sha256:{hash}")], "complete": [format!("sha256:{hash}")], "ttl": 600
        }))
        .unwrap(),
      ))
      .unwrap();
    let resp = app.oneshot(announce_req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // query without session auth: peers are still returned (it is a lookup)
    let app2 = router(state, config());
    let req = Request::builder()
      .method("GET")
      .uri(format!("/resources/sha256:{hash}/peers"))
      .body(Body::empty())
      .unwrap();
    let resp = app2.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["peers"][0]["peer_id"], peer);
    assert_eq!(
      json["peers"][0]["punch_server"],
      "https://punch-1.example.com"
    );
    assert_eq!(json["peers"][0]["complete"], true);
  }

  #[tokio::test]
  async fn query_rejects_a_malformed_info_hash() {
    let app = router(Arc::new(MemoryState::new()), config());
    let req = Request::builder()
      .method("GET")
      .uri("/resources/not-a-hash/peers")
      .body(Body::empty())
      .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
  }

  #[tokio::test]
  async fn turn_credential_follows_coturn_rest() {
    let cfg = config();
    let wire = turn_credential(&cfg, "peer-1", 1_700_000_000);
    assert_eq!(wire.username, "1700000000:peer-1");
    // base64 of HMAC-SHA1 under the shared secret
    let expected = Hmac::<Sha1>::new_from_slice(b"test-secret")
      .unwrap()
      .chain_update(wire.username.as_bytes())
      .finalize()
      .into_bytes();
    assert_eq!(wire.credential, B64.encode(expected));
  }
}
