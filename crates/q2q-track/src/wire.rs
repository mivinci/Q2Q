//! JSON shapes shared by the Track server and its clients.
//!
//! These types describe the HTTP boundary only. Authentication and storage
//! decisions stay in the server; clients use the same field names and formats.

use crate::state::UnixSeconds;
use q2q_primitives::identity::TicketBody;
use serde::{Deserialize, Serialize};

/// A stable code clients can branch on, with optional diagnostic detail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBody {
  pub code: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnnounceResponse {
  pub session_key: String,
  pub punch: PunchWire,
  pub turn: TurnWire,
  pub heartbeat_interval: u32,
  pub expires_at: UnixSeconds,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PunchWire {
  pub punch_id: String,
  pub url: String,
  pub public_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnWire {
  pub url: String,
  pub username: String,
  pub credential: String,
  pub expires_at: UnixSeconds,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatRequest {
  pub seq: u64,
  pub timestamp: UnixSeconds,
  pub mac: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatResponse {
  pub expires_at: UnixSeconds,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
  #[serde(default)]
  /// Maximum requested candidates, not a promise to fill the response.
  pub limit: Option<usize>,
  #[serde(default)]
  pub cursor: Option<String>,
  /// Query has no body; the session HMAC travels in the URL parameters.
  pub peer_id: Option<String>,
  pub seq: Option<u64>,
  pub timestamp: Option<UnixSeconds>,
  pub mac: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResponse {
  pub peers: Vec<PeerWire>,
  pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerWire {
  pub peer_id: String,
  pub public_key: String,
  /// Resolved from the current Punch list for each query. Addresses can move.
  pub punch: PunchWire,
  pub complete: bool,
}

/// HTTP query limits shared by services and SDKs.
pub const DEFAULT_QUERY_LIMIT: usize = 10;
pub const MAX_QUERY_LIMIT: usize = 50;
