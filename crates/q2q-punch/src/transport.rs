//! Transport-independent frames used by stream transports such as WebSocket.
//!
//! QUIC keeps its existing RPC stream/datagram wire format. A WebSocket is one
//! ordered byte stream, so it needs a discriminator for RPC requests,
//! responses, and server-pushed signals.

use crate::message::Request;
use crate::message::Response;
use crate::signal::SignalRequest;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum PunchFrame {
  Request(Request),
  Response(Response),
  Signal(SignalRequest),
}
