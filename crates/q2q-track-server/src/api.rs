//! Peer HTTP endpoints, public service health, and TURN credential minting.
//!
//! Track never issues identity tickets — clients sign their own. The one thing
//! it *does* issue is a TURN credential, because relay capacity costs money and
//! has to be rate limited somewhere.

#[cfg(test)]
use crate::directory::StaticPunchDirectory as PunchList;
use crate::directory::{DirectoryError, PunchDirectory};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use q2q_primitives::identity::PeerId;
use q2q_ratelimit::{Limiter, RateLimitLayer, Rule};
use q2q_track::auth::{self, AuthError};
#[cfg(test)]
use q2q_track::state::PunchAssignment;
use q2q_track::state::{InfoHash, StateError, TrackState, UnixSeconds};
use q2q_track::wire::{
  AnnounceRequest, AnnounceResponse, ErrorBody, HeartbeatRequest, HeartbeatResponse, PeerWire,
  PunchWire, QueryParams, QueryResponse, TurnWire,
};
use std::collections::HashMap;
use std::sync::Arc;

pub use q2q_track::wire::{DEFAULT_QUERY_LIMIT as DEFAULT_LIMIT, MAX_QUERY_LIMIT as MAX_LIMIT};
pub const DEFAULT_HEARTBEAT_INTERVAL: u32 = 30;

/// Where Track gets its Punch list and TURN secret from.
#[derive(Debug, Clone)]
pub struct TrackConfig {
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
  /// Replaceable at runtime: an instance can be moved or retired, and the next
  /// query has to hand out whatever is true now.
  pub punch: Arc<dyn PunchDirectory>,
}

// Hand-written: deriving Clone would require `S: Clone`, which state backends
// are not (and need not be — they are shared behind an Arc).
impl<S: TrackState> Clone for AppState<S> {
  fn clone(&self) -> Self {
    AppState {
      state: self.state.clone(),
      config: self.config.clone(),
      punch: self.punch.clone(),
    }
  }
}

/// One rule per endpoint class. Announce has to verify an Ed25519 signature and
/// write the index, so it costs far more than a query.
pub use q2q_ratelimit::rules::{TRACK_ANNOUNCE as RULE_ANNOUNCE, TRACK_QUERY as RULE_QUERY};

pub fn limiter() -> Arc<Limiter> {
  let mut rules = HashMap::new();
  rules.insert(RULE_ANNOUNCE.to_string(), Rule::per_minute(10, 10));
  rules.insert(RULE_QUERY.to_string(), Rule::per_minute(60, 60));
  Arc::new(Limiter::new(rules))
}

/// Every part explicit: a reload swaps the Punch list, and tests want to do
/// that without touching the config.
pub fn router_with<S: TrackState + 'static>(
  state: Arc<S>,
  config: Arc<TrackConfig>,
  punch: Arc<dyn PunchDirectory>,
  limits: Arc<Limiter>,
) -> Router {
  let app = AppState {
    state,
    config,
    punch,
  };

  // Separate routers so each class gets its own rule; a single route_layer at
  // the end would stack every layer onto every route.
  let announce_route = Router::new()
    .route("/peers/{peer_id}/files", put(announce::<S>))
    .route_layer(RateLimitLayer::new(limits.clone(), RULE_ANNOUNCE));

  let read_routes = Router::new()
    .route(
      "/health",
      get(|| async {
        // Only public service identity is readable cross-origin; other APIs keep their policy.
        (
          [
            ("access-control-allow-origin", "*"),
            ("cache-control", "no-store"),
          ],
          Json(serde_json::json!({ "service": "q2q-track", "version": env!("CARGO_PKG_VERSION") })),
        )
      }),
    )
    .route("/peers/{peer_id}/heartbeat", post(heartbeat::<S>))
    .route("/files/{info_hash}/peers", get(query::<S>))
    .route_layer(RateLimitLayer::new(limits.clone(), RULE_QUERY));

  Router::new()
    .merge(announce_route)
    .merge(read_routes)
    .with_state(app)
}

fn fail(code: &'static str, detail: Option<String>) -> (StatusCode, Json<ErrorBody>) {
  let status = match code {
    "malformed_ticket" | "bad_request" => StatusCode::BAD_REQUEST,
    "bad_signature" | "key_not_issuer" | "expired" | "wrong_audience" | "bad_mac"
    | "stale_timestamp" => StatusCode::UNAUTHORIZED,
    "unknown_session" => StatusCode::UNAUTHORIZED,
    "storage_unavailable" | "discovery_unavailable" | "no_available_punch" => {
      StatusCode::SERVICE_UNAVAILABLE
    }
    _ => StatusCode::INTERNAL_SERVER_ERROR,
  };

  // one place decides how loud a rejection is: a malformed request from a
  // client is routine, a failed signature is someone probing, a 5xx is ours
  match status {
    StatusCode::INTERNAL_SERVER_ERROR | StatusCode::SERVICE_UNAVAILABLE => {
      tracing::error!(code, "request failed")
    }
    StatusCode::UNAUTHORIZED => tracing::warn!(code, "request rejected"),
    _ => tracing::debug!(code, ?detail, "request rejected"),
  }
  (
    status,
    Json(ErrorBody {
      code: code.into(),
      detail,
    }),
  )
}

fn directory_failure(_: DirectoryError) -> (StatusCode, Json<ErrorBody>) {
  // Background refresh logs discovery failures; avoid one error log per Peer request.
  (
    StatusCode::SERVICE_UNAVAILABLE,
    Json(ErrorBody {
      code: "discovery_unavailable".into(),
      detail: None,
    }),
  )
}

fn auth_failure(e: AuthError) -> (StatusCode, Json<ErrorBody>) {
  let detail = match &e {
    AuthError::MalformedTicket(d) => Some(d.clone()),
    _ => None,
  };
  // fail() logs it; auth rejections are the interesting kind
  fail(e.code(), detail)
}

fn state_failure(e: StateError) -> (StatusCode, Json<ErrorBody>) {
  match e {
    StateError::UnknownSession => fail("unknown_session", None),
    StateError::Replayed(_) => fail("bad_mac", None),
    StateError::Unavailable(detail) => {
      tracing::error!(%detail, "Track state unavailable");
      fail("storage_unavailable", None)
    }
  }
}

// ------------------------------------------------------------------ announce

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

  let Some(punch) = app.punch.assign(&ticket.iss).map_err(directory_failure)? else {
    // no Punch Server listed means nobody can be reached; say so rather than
    // handing out an empty address
    return Err(fail("no_available_punch", None));
  };

  let session_key = match random_32() {
    Ok(k) => k,
    Err(e) => {
      // not the client's fault: nothing about its ticket was wrong
      tracing::error!("could not mint a session key: {e}");
      return Err(fail(
        "internal_error",
        Some("session key unavailable".into()),
      ));
    }
  };
  app
    .state
    .announce(
      q2q_track::state::OnlineRecord {
        peer_id: ticket.iss.clone(),
        public_key: ticket.public_key,
        session_key,
        last_seq: 0,
        punch: punch.clone(),
        expires_at,
      },
      &add,
      &del,
      &complete,
      ttl,
    )
    .await
    .map_err(state_failure)?;

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
  let credential = q2q_primitives::mac::sha1(config.turn_secret.as_bytes(), &[username.as_bytes()]);
  TurnWire {
    url: config.turn_url.clone(),
    username,
    credential: B64.encode(credential),
    expires_at,
  }
}

fn random_32() -> Result<[u8; 32], getrandom::Error> {
  let mut out = [0u8; 32];
  getrandom::fill(&mut out)?;
  Ok(out)
}

// ----------------------------------------------------------------- heartbeat

async fn heartbeat<S: TrackState>(
  State(app): State<AppState<S>>,
  Path(peer_id): Path<String>,
  Json(req): Json<HeartbeatRequest>,
) -> Result<Json<HeartbeatResponse>, (StatusCode, Json<ErrorBody>)> {
  let now = q2q_track::state::now();
  let peer = PeerId::unchecked(peer_id);

  let record = app
    .state
    .online(&peer, now)
    .await
    .map_err(state_failure)?
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

  if app.punch.requires_matching_identity() {
    let current = app
      .punch
      .get(&record.punch.punch_id)
      .map_err(directory_failure)?;
    if current.as_ref() != Some(&record.punch) {
      // Reuse the SDK's existing reannounce path; never migrate a binding by editing its ID.
      return Err(fail("unknown_session", None));
    }
  }

  let ttl = app.config.max_ttl.min(3600);
  let expires_at = now + ttl;
  app
    .state
    .advance_session(&peer, &record.session_key, req.seq, Some(expires_at), now)
    .await
    .map_err(state_failure)?;

  Ok(Json(HeartbeatResponse { expires_at }))
}

// --------------------------------------------------------------------- query

async fn query<S: TrackState>(
  State(app): State<AppState<S>>,
  Path(info_hash): Path<String>,
  Query(params): Query<QueryParams>,
) -> Result<Json<QueryResponse>, (StatusCode, Json<ErrorBody>)> {
  let now = q2q_track::state::now();

  // 先把请求本身的形状查清楚：格式错的请求不必走到认证那一步
  let hash = parse_info_hash(&info_hash)
    .ok_or_else(|| fail("bad_request", Some("info_hash must be sha256:<hex>".into())))?;

  // spec 7.2.1 says query carries the session; there is no body, so the
  // HMAC triple rides along as query parameters. All four are required —
  // letting a request through without them (as an `if let` would) makes query
  // an anonymous enumeration endpoint for peer ids and public keys.
  let (pid, seq, ts, mac) = match (
    params.peer_id.as_deref(),
    params.seq,
    params.timestamp,
    params.mac.as_deref(),
  ) {
    (Some(pid), Some(seq), Some(ts), Some(mac)) => (pid, seq, ts, mac),
    _ => {
      return Err(fail(
        "unknown_session",
        Some("query needs peer_id, seq, timestamp and mac".into()),
      ));
    }
  };

  let peer = PeerId::unchecked(pid.to_string());
  let record = app
    .state
    .online(&peer, now)
    .await
    .map_err(state_failure)?
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
    .advance_session(&peer, &record.session_key, seq, None, now)
    .await
    .map_err(state_failure)?;

  let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
  // Resolve one Punch snapshot for the whole selection and response.
  let snapshot = app.punch.snapshot().map_err(directory_failure)?;
  let punches = snapshot.nodes;
  let punch_ids = punches.keys().cloned().collect();
  let mut entropy = [0; 32];
  getrandom::fill(&mut entropy).map_err(|_| {
    (
      StatusCode::INTERNAL_SERVER_ERROR,
      Json(ErrorBody {
        code: "internal".into(),
        detail: None,
      }),
    )
  })?;
  let selection = q2q_track::selection::Query {
    info_hash: &hash,
    requester: &peer,
    limit,
    cursor: params.cursor.as_deref(),
    punch_ids: &punch_ids,
    now,
  }
  .select(
    &*app.state,
    q2q_primitives::identity::peer_id_from_public_key(&entropy),
  )
  .await
  .map_err(|error| match error {
    q2q_track::selection::SelectionError::State(error) => state_failure(error),
    _ => fail("bad_request", Some("invalid query cursor or limit".into())),
  })?;
  let mut compatible = Vec::new();
  for candidate in selection.peers {
    if snapshot.strict_identity {
      let Some(record) = app
        .state
        .online(&candidate.peer_id, now)
        .await
        .map_err(state_failure)?
      else {
        continue;
      };
      if record.punch != punches[&candidate.punch_id] {
        continue;
      }
    }
    compatible.push(candidate);
  }
  let peers = compatible
    .into_iter()
    .map(|c| {
      let punch = &punches[&c.punch_id];
      PeerWire {
        peer_id: c.peer_id.to_string(),
        public_key: B64.encode(c.public_key),
        punch: PunchWire {
          punch_id: punch.punch_id.clone(),
          url: punch.url.clone(),
          public_key: B64.encode(&punch.public_key),
        },
        complete: c.complete,
      }
    })
    .collect();
  let next_cursor = selection.next_cursor;

  Ok(Json(QueryResponse { peers, next_cursor }))
}

#[cfg(test)]
mod tests {
  use super::*;
  use axum::body::Body;
  use axum::http::{Request, StatusCode};
  use q2q_primitives::identity::{
    TICKET_VERSION, Ticket, TicketBody, keypair_from_seed, peer_id_of,
  };
  use q2q_ratelimit::PeerAddr;
  use q2q_track::state::MemoryState;
  use tower::ServiceExt;

  /// The list is the only source of Punch addresses now.
  fn build(state: Arc<MemoryState>, punch: Arc<PunchList>) -> Router {
    router_with(state, config(), punch, limiter())
  }

  fn build_with_limits(
    state: Arc<MemoryState>,
    punch: Arc<PunchList>,
    limits: Arc<Limiter>,
  ) -> Router {
    router_with(state, config(), punch, limits)
  }

  /// query 现在要求会话（spec 7.2.1），所以测试先 announce 一次拿 session_key。
  async fn session_with(
    app: &Router,
    seed: u8,
    add: &[String],
    complete: &[String],
  ) -> (String, [u8; 32]) {
    let body = ticket_body(seed, "track");
    let peer = body.iss.clone();
    let req = Request::builder()
      .method("PUT")
      .uri(format!("/peers/{peer}/files"))
      .header("content-type", "application/json")
      .body(Body::from(
        serde_json::to_vec(&serde_json::json!({
          "token": body, "add": add, "complete": complete, "ttl": 600
        }))
        .unwrap(),
      ))
      .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let key = B64.decode(json["session_key"].as_str().unwrap()).unwrap();
    (peer, key.try_into().expect("session key is 32 bytes"))
  }

  async fn session_for(app: &Router, seed: u8) -> (String, [u8; 32]) {
    session_with(app, seed, &[], &[]).await
  }

  /// base64 里的 `+` `/` `=` 在 query string 中另有含义，得转义
  fn pct(s: &str) -> String {
    s.replace('+', "%2B")
      .replace('/', "%2F")
      .replace('=', "%3D")
  }

  /// 带会话三元组的 query URI。`seq` 必须逐次递增。
  fn authed_query(hash: &str, peer: &str, key: &[u8; 32], seq: u64) -> String {
    let ts = q2q_track::state::now();
    let mac = B64.encode(q2q_track::auth::mac_bytes(
      key,
      &PeerId::unchecked(peer.to_string()),
      seq,
      ts,
    ));
    format!(
      "/files/sha256:{hash}/peers?peer_id={peer}&seq={seq}&timestamp={ts}&mac={}",
      pct(&mac)
    )
  }

  fn punch_list() -> Arc<PunchList> {
    punch_list_at("https://punch-1.example.com")
  }

  fn punch_list_at(url: &str) -> Arc<PunchList> {
    PunchList::new(vec![PunchAssignment {
      punch_id: "punch-1".into(),
      url: url.into(),
      public_key: vec![7u8; 32],
    }])
  }

  fn config() -> Arc<TrackConfig> {
    Arc::new(TrackConfig {
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
      v: TICKET_VERSION,
      iss: peer_id_of(&sk),
      public_key: sk.verifying_key().to_bytes(),
      iat: q2q_track::state::now(),
      exp: q2q_track::state::now() + 600,
      aud: aud.to_string(),
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
    let app = build(Arc::new(MemoryState::new()), punch_list());
    let body = ticket_body(1, "track");
    let peer = body.iss.clone();
    let req = Request::builder()
      .method("PUT")
      .uri(format!("/peers/{peer}/files"))
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
    let app = build(Arc::new(MemoryState::new()), punch_list());
    let body = ticket_body(2, "punch-9");
    let peer = body.iss.clone();
    let req = Request::builder()
      .method("PUT")
      .uri(format!("/peers/{peer}/files"))
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
    let app = build(Arc::new(MemoryState::new()), punch_list());
    let body = ticket_body(3, "track");
    let peer = body.iss.clone();
    let req = Request::builder()
      .method("PUT")
      .uri(format!("/peers/{peer}/files"))
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
    let app = build(Arc::new(MemoryState::new()), punch_list());
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
    let app = build(state.clone(), punch_list());
    let hash = "ab".repeat(32);
    let (holder, _) = session_with(
      &app,
      4,
      &[format!("sha256:{hash}")],
      &[format!("sha256:{hash}")],
    )
    .await;

    let (peer, key) = session_for(&app, 200).await;
    let app2 = build(state, punch_list());
    let req = Request::builder()
      .method("GET")
      .uri(authed_query(&hash, &peer, &key, 1))
      .body(Body::empty())
      .unwrap();
    let resp = app2.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["peers"][0]["peer_id"], holder);
    // the address and key come from the live list, never from a snapshot
    assert_eq!(json["peers"][0]["punch"]["punch_id"], "punch-1");
    assert_eq!(
      json["peers"][0]["punch"]["url"],
      "https://punch-1.example.com"
    );
    assert_eq!(
      json["peers"][0]["punch"]["public_key"],
      B64.encode(vec![7u8; 32])
    );
    assert_eq!(json["peers"][0]["complete"], true);
  }

  /// The point of resolving at query time: if the instance moves, the next
  /// query hands out the new address. A snapshot would keep the old one.
  #[tokio::test]
  async fn query_backfills_the_address_from_the_live_list() {
    let state = Arc::new(MemoryState::new());
    let hash = "ab".repeat(32);

    let announce_app = build(state.clone(), punch_list_at("https://old.example.com"));
    let (_holder, _) = session_with(&announce_app, 3, &[format!("sha256:{hash}")], &[]).await;

    let (peer, key) = session_for(&announce_app, 201).await;

    // same peer, but Track's list now points punch-1 somewhere else
    let app = build(state, punch_list_at("https://new.example.com"));
    let req = Request::builder()
      .method("GET")
      .uri(authed_query(&hash, &peer, &key, 1))
      .body(Body::empty())
      .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let json = body_json(resp).await;
    assert_eq!(json["peers"][0]["punch"]["url"], "https://new.example.com");
  }

  #[tokio::test]
  async fn a_peer_bound_to_a_retired_punch_is_not_returned() {
    let state = Arc::new(MemoryState::new());
    let hash = "cd".repeat(32);

    let announce_app = build(state.clone(), punch_list());
    let (_holder, _) = session_with(&announce_app, 4, &[format!("sha256:{hash}")], &[]).await;

    let (peer, key) = session_for(&announce_app, 202).await;

    // punch-1 is gone from the list: nobody can reach this peer any more
    let retired = PunchList::new(vec![PunchAssignment {
      punch_id: "punch-2".into(),
      url: "https://punch-2.example.com".into(),
      public_key: vec![8u8; 32],
    }]);

    let app = build(state, retired);
    let req = Request::builder()
      .method("GET")
      .uri(authed_query(&hash, &peer, &key, 1))
      .body(Body::empty())
      .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let json = body_json(resp).await;
    assert!(
      json["peers"].as_array().unwrap().is_empty(),
      "a peer whose Punch Server is gone cannot be reached"
    );
  }

  /// A reload swaps the list in place; the next query must hand out the new
  /// address, with no restart and no client-side caching.
  #[tokio::test]
  async fn replacing_the_punch_list_changes_what_query_hands_out() {
    let state = Arc::new(MemoryState::new());
    let hash = "ef".repeat(32);
    let punch = PunchList::new(vec![PunchAssignment {
      punch_id: "punch-1".into(),
      url: "https://before.example.com".into(),
      public_key: vec![7u8; 32],
    }]);
    let announce_app = build(state.clone(), punch.clone());
    let (_holder, _) = session_with(&announce_app, 5, &[format!("sha256:{hash}")], &[]).await;

    let (peer, key) = session_for(&announce_app, 203).await;

    // seq 必须逐次递增，所以每次查询都要带一个
    let query = |seq: u64| {
      let state = state.clone();
      let punch = punch.clone();
      let hash = hash.clone();
      let peer = peer.clone();
      async move {
        let app = build(state, punch);
        let req = Request::builder()
          .method("GET")
          .uri(authed_query(&hash, &peer, &key, seq))
          .body(Body::empty())
          .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        body_json(resp).await
      }
    };

    assert_eq!(
      query(1).await["peers"][0]["punch"]["url"],
      "https://before.example.com"
    );

    punch.replace(vec![PunchAssignment {
      punch_id: "punch-1".into(),
      url: "https://after.example.com".into(),
      public_key: vec![7u8; 32],
    }]);
    let json = query(2).await;
    assert_eq!(
      json["peers"][0]["punch"]["url"],
      "https://after.example.com"
    );
    // the identity outlives the address
    assert_eq!(
      json["peers"][0]["punch"]["public_key"],
      B64.encode(vec![7u8; 32])
    );
  }

  fn tiny_limiter(rule: &str, capacity: u32) -> Arc<Limiter> {
    let mut rules = HashMap::new();
    rules.insert(rule.to_string(), Rule::per_minute(capacity, 60));
    Arc::new(Limiter::new(rules))
  }

  #[tokio::test]
  async fn a_peer_that_exceeds_the_bucket_gets_429() {
    let state = Arc::new(MemoryState::new());
    let app = build_with_limits(state, punch_list(), tiny_limiter(RULE_QUERY, 2));
    let (peer, key) = session_for(&app, 1).await;

    let hash = "ab".repeat(32);
    for seq in [1u64, 2] {
      let uri = authed_query(&hash, &peer, &key, seq);
      let mut req = Request::builder()
        .method("GET")
        .uri(&uri)
        .body(Body::empty())
        .unwrap();
      req
        .extensions_mut()
        .insert(PeerAddr("198.51.100.7".parse().unwrap()));
      let resp = app.clone().oneshot(req).await.unwrap();
      assert_eq!(resp.status(), StatusCode::OK, "seq {seq}");
    }

    let uri = authed_query(&hash, &peer, &key, 3);
    let mut req = Request::builder()
      .method("GET")
      .uri(&uri)
      .body(Body::empty())
      .unwrap();
    req
      .extensions_mut()
      .insert(PeerAddr("198.51.100.7".parse().unwrap()));
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(resp.headers().contains_key(axum::http::header::RETRY_AFTER));
  }

  #[tokio::test]
  async fn a_different_network_keeps_its_own_bucket() {
    let state = Arc::new(MemoryState::new());
    let app = build_with_limits(state, punch_list(), tiny_limiter(RULE_QUERY, 1));
    let (peer, key) = session_for(&app, 2).await;

    let hash = "ab".repeat(32);
    for (seq, ip) in [(1u64, "198.51.100.7"), (2, "203.0.113.7")] {
      let uri = authed_query(&hash, &peer, &key, seq);
      let mut req = Request::builder()
        .method("GET")
        .uri(&uri)
        .body(Body::empty())
        .unwrap();
      req.extensions_mut().insert(PeerAddr(ip.parse().unwrap()));
      let resp = app.clone().oneshot(req).await.unwrap();
      assert_eq!(resp.status(), StatusCode::OK, "ip {ip}");
    }
  }

  #[tokio::test]
  async fn query_rejects_a_malformed_info_hash() {
    let app = build(Arc::new(MemoryState::new()), punch_list());
    let req = Request::builder()
      .method("GET")
      .uri("/files/not-a-hash/peers")
      .body(Body::empty())
      .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
  }

  #[tokio::test]
  async fn query_without_a_session_is_rejected() {
    // 匿名 query 等于把 peer_id 与公钥开放给任何人枚举（spec 7.2.1）
    let app = build(Arc::new(MemoryState::new()), punch_list());
    let hash = "ab".repeat(32);
    let req = Request::builder()
      .method("GET")
      .uri(format!("/files/sha256:{hash}/peers"))
      .body(Body::empty())
      .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(body_json(resp).await["code"], "unknown_session");
  }

  #[tokio::test]
  async fn query_with_a_partial_session_is_rejected() {
    let app = build(Arc::new(MemoryState::new()), punch_list());
    let hash = "ab".repeat(32);
    // 四个参数缺两个：不能当成"没有会话"就放行
    let req = Request::builder()
      .method("GET")
      .uri(format!("/files/sha256:{hash}/peers?peer_id=whatever&seq=1"))
      .body(Body::empty())
      .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
  }

  #[tokio::test]
  async fn replaying_a_heartbeat_sequence_is_rejected() {
    let state = Arc::new(MemoryState::new());
    let app = build(state, punch_list());
    let (peer, key) = session_for(&app, 6).await;

    // 同一个 seq 连发两次。旧实现把 touch_sequence 之前的旧 last_seq 又写了
    // 回去，递增校验等于被自己撤销，第二次会当成合法请求
    let mut statuses = Vec::new();
    for _ in 0..2 {
      let ts = q2q_track::state::now();
      let mac = B64.encode(q2q_track::auth::mac_bytes(
        &key,
        &PeerId::unchecked(peer.clone()),
        1,
        ts,
      ));
      let req = Request::builder()
        .method("POST")
        .uri(format!("/peers/{peer}/heartbeat"))
        .header("content-type", "application/json")
        .body(Body::from(
          serde_json::to_vec(&serde_json::json!({"seq": 1, "timestamp": ts, "mac": mac})).unwrap(),
        ))
        .unwrap();
      let resp = app.clone().oneshot(req).await.unwrap();
      statuses.push(resp.status());
    }

    assert_eq!(statuses[0], StatusCode::OK);
    assert_eq!(statuses[1], StatusCode::UNAUTHORIZED);
  }

  #[tokio::test]
  async fn a_heartbeat_that_advances_the_sequence_is_accepted() {
    // 顺便确认正常递增没有被上面的修改弄坏
    let state = Arc::new(MemoryState::new());
    let app = build(state, punch_list());
    let (peer, key) = session_for(&app, 7).await;

    for seq in [1u64, 2, 3] {
      let ts = q2q_track::state::now();
      let mac = B64.encode(q2q_track::auth::mac_bytes(
        &key,
        &PeerId::unchecked(peer.clone()),
        seq,
        ts,
      ));
      let req = Request::builder()
        .method("POST")
        .uri(format!("/peers/{peer}/heartbeat"))
        .header("content-type", "application/json")
        .body(Body::from(
          serde_json::to_vec(&serde_json::json!({"seq": seq, "timestamp": ts, "mac": mac}))
            .unwrap(),
        ))
        .unwrap();
      let resp = app.clone().oneshot(req).await.unwrap();
      assert_eq!(resp.status(), StatusCode::OK, "seq {seq}");
    }
  }

  #[tokio::test]
  async fn turn_credential_follows_coturn_rest() {
    let cfg = config();
    let wire = turn_credential(&cfg, "peer-1", 1_700_000_000);
    assert_eq!(wire.username, "1700000000:peer-1");
    // base64 of HMAC-SHA1 under the shared secret
    let expected = q2q_primitives::mac::sha1(b"test-secret", &[wire.username.as_bytes()]);
    assert_eq!(wire.credential, B64.encode(expected));
  }

  #[tokio::test]
  #[ignore = "requires Q2Q_REDIS_TEST_HOST and Q2Q_REDIS_TEST_PASSWORD"]
  async fn redis_routes_share_sessions_across_track_instances() {
    let host = std::env::var("Q2Q_REDIS_TEST_HOST").expect("Redis test host");
    let password = std::env::var("Q2Q_REDIS_TEST_PASSWORD").expect("Redis test password");
    let encoded: String = password
      .as_bytes()
      .iter()
      .map(|byte| format!("%{byte:02X}"))
      .collect();
    let url = format!("redis://:{encoded}@{host}/");
    let prefix = format!(
      "q2q:route-probe:{}:{}:",
      std::process::id(),
      std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
    );
    let first = Arc::new(
      q2q_track::redis::RedisState::connect_with_prefix(&url, &prefix)
        .await
        .unwrap(),
    );
    let second = Arc::new(
      q2q_track::redis::RedisState::connect_with_prefix(&url, &prefix)
        .await
        .unwrap(),
    );
    let app_a = router_with(first, config(), punch_list(), limiter());
    let app_b = router_with(second, config(), punch_list(), limiter());
    let hash = "ab".repeat(32);
    let (peer, key) = session_with(&app_a, 19, &[format!("sha256:{hash}")], &[]).await;

    let (holder, _) = session_with(&app_a, 20, &[format!("sha256:{hash}")], &[]).await;

    let req = Request::builder()
      .method("GET")
      .uri(authed_query(&hash, &peer, &key, 1))
      .body(Body::empty())
      .unwrap();
    let resp = app_b.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_json(resp).await["peers"][0]["peer_id"], holder);

    let ts = q2q_track::state::now();
    let mac = B64.encode(q2q_track::auth::mac_bytes(
      &key,
      &PeerId::unchecked(peer.clone()),
      2,
      ts,
    ));
    let req = Request::builder()
      .method("POST")
      .uri(format!("/peers/{peer}/heartbeat"))
      .header("content-type", "application/json")
      .body(Body::from(
        serde_json::to_vec(&serde_json::json!({"seq": 2, "timestamp": ts, "mac": mac})).unwrap(),
      ))
      .unwrap();
    assert_eq!(app_b.oneshot(req).await.unwrap().status(), StatusCode::OK);

    let req = Request::builder()
      .method("GET")
      .uri(authed_query(&hash, &peer, &key, 3))
      .body(Body::empty())
      .unwrap();
    assert_eq!(app_a.oneshot(req).await.unwrap().status(), StatusCode::OK);

    let client = redis::Client::open(url.as_str()).unwrap();
    let mut conn = client.get_connection_manager().await.unwrap();
    let _: u64 = redis::cmd("DEL")
      .arg(format!("{prefix}online:{peer}"))
      .arg(format!("{prefix}online:{holder}"))
      .arg(format!("{prefix}file:{hash}:members"))
      .arg(format!("{prefix}file:{hash}:meta"))
      .arg(format!("{prefix}holding-expiry"))
      .query_async(&mut conn)
      .await
      .unwrap();
  }

  #[tokio::test]
  async fn query_limits_and_opaque_pagination_exclude_requester() {
    let state = Arc::new(MemoryState::new());
    let app = build(state.clone(), punch_list());
    let hash = "12".repeat(32);
    let hash_text = format!("sha256:{hash}");
    let (peer, key) = session_with(&app, 220, std::slice::from_ref(&hash_text), &[]).await;
    for seed in 1..=12 {
      session_with(&app, seed, std::slice::from_ref(&hash_text), &[]).await;
    }
    let mut cursor = None;
    let mut seen = std::collections::HashSet::new();
    for seq in 1..=8 {
      let mut uri = format!("{}&limit=2", authed_query(&hash, &peer, &key, seq));
      if let Some(cursor) = &cursor {
        uri.push_str(&format!("&cursor={cursor}"));
      }
      let response = app
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
      assert_eq!(response.status(), StatusCode::OK);
      let json = body_json(response).await;
      let peers = json["peers"].as_array().unwrap();
      assert!(peers.len() <= 2);
      for candidate in peers {
        let id = candidate["peer_id"].as_str().unwrap().to_owned();
        assert_ne!(id, peer);
        assert!(seen.insert(id));
      }
      cursor = json["next_cursor"].as_str().map(str::to_owned);
      if cursor.is_none() {
        break;
      }
    }
    assert_eq!(seen.len(), 12);
    assert!(cursor.is_none());
    for (seq, suffix, expected) in [(9, "", 10), (10, "&limit=0", 1), (11, "&limit=999999", 12)] {
      let uri = format!("{}{suffix}", authed_query(&hash, &peer, &key, seq));
      let response = app
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
      assert_eq!(
        body_json(response).await["peers"].as_array().unwrap().len(),
        expected
      );
    }
    let uri = format!("{}&cursor=v1.bad", authed_query(&hash, &peer, &key, 12));
    let response = app
      .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
      .await
      .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
  }

  #[tokio::test]
  async fn query_backfills_beyond_retired_punch_prefix() {
    let state = Arc::new(MemoryState::new());
    let app = build(state.clone(), punch_list());
    let (requester, session_key) = session_for(&app, 250).await;
    let now = q2q_track::state::now();
    let mut holders: Vec<_> = (1u8..=90)
      .map(|seed| {
        let (key, id) = q2q_primitives::identity::keypair_from_seed(&[seed; 32]);
        (id, q2q_primitives::identity::public_key_of(&key))
      })
      .collect();
    holders.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
    // The first 80 sorted holders use a retired Punch; all 10 good peers must
    // remain discoverable even when requesting only three candidates.
    for (index, (id, key)) in holders.iter().enumerate() {
      state
        .announce(
          q2q_track::state::OnlineRecord {
            peer_id: id.clone(),
            public_key: *key,
            session_key: [1; 32],
            last_seq: 0,
            punch: PunchAssignment {
              punch_id: if index < 80 { "retired" } else { "punch-1" }.into(),
              url: "quic://unused".into(),
              public_key: vec![7; 32],
            },
            expires_at: now + 600,
          },
          &[[0x34; 32]],
          &[],
          &[],
          600,
        )
        .await
        .unwrap();
    }
    // A legacy cursor fixes the scan before the invalid prefix, avoiding any
    // dependence on the random starting position of a fresh request.
    let legacy = holders[0].0.to_string();
    let uri = format!(
      "{}&limit=3&cursor={legacy}",
      authed_query(&"34".repeat(32), &requester, &session_key, 1)
    );
    let response = app
      .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
      .await
      .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["peers"].as_array().unwrap().len(), 3);
    assert!(json["next_cursor"].as_str().is_some());
    assert!(
      json["peers"]
        .as_array()
        .unwrap()
        .iter()
        .all(|peer| peer["punch"]["punch_id"] == "punch-1")
    );
  }
}
