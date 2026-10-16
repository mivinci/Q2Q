//! Rate limits for the RPCs.
//!
//! QUIC gives us no tower layer to hang a filter off, so the handler asks
//! explicitly. Same limiter as Track's, same reasoning: keyed on network,
//! because identities are free to mint.

use q2q_punch::message::{Method, Request};
use q2q_punch::signal::SignalType;
use q2q_ratelimit::limiter::Millis;
use q2q_ratelimit::rules::{PUNCH_CONNECT, PUNCH_HEARTBEAT, PUNCH_JOIN, PUNCH_SIGNAL};
use q2q_ratelimit::{Key, Limiter, NetworkKey, Rule, Verdict};
use std::collections::HashMap;
use std::net::IpAddr;

/// Defaults, per network prefix, per minute.
pub const JOIN_PER_MINUTE: u32 = 10;
pub const HEARTBEAT_PER_MINUTE: u32 = 120;
pub const CONNECT_PER_MINUTE: u32 = 20;
pub const SIGNAL_PER_MINUTE: u32 = 120;

/// Every call site passes its own numbers: from config in production, or a
/// bucket small enough to observe in a test.
pub fn limiter_with(join: u32, heartbeat: u32, connect: u32, signal: u32) -> Limiter {
  let mut rules = HashMap::new();
  rules.insert(PUNCH_JOIN.to_string(), Rule::per_minute(join, join));
  rules.insert(
    PUNCH_HEARTBEAT.to_string(),
    Rule::per_minute(heartbeat, heartbeat),
  );
  rules.insert(
    PUNCH_CONNECT.to_string(),
    Rule::per_minute(connect, connect),
  );
  rules.insert(PUNCH_SIGNAL.to_string(), Rule::per_minute(signal, signal));
  Limiter::new(rules)
}

/// Which rule a request is charged against, and whether it is narrowed to one
/// file.
///
/// Waking a peer is the one call that lands on somebody else, so `connect` is
/// scoped per file as well: a hot file should not be swarmed by one prefix.
pub fn classify(req: &Request) -> (&'static str, Option<[u8; 32]>) {
  let Some(method) = Method::parse(&req.method) else {
    return (PUNCH_SIGNAL, None);
  };

  match method {
    Method::Join | Method::Exit => (PUNCH_JOIN, None),
    Method::Heartbeat => (PUNCH_HEARTBEAT, None),
    Method::Signal => match signal_kind(req) {
      Some(SignalType::Connect) => (PUNCH_CONNECT, connect_file(req)),
      _ => (PUNCH_SIGNAL, None),
    },
  }
}

fn signal_kind(req: &Request) -> Option<SignalType> {
  let kind = req.params.get("type")?.as_str()?;
  match kind {
    "connect" => Some(SignalType::Connect),
    "accept" => Some(SignalType::Accept),
    "reject" => Some(SignalType::Reject),
    "offer" => Some(SignalType::Offer),
    "answer" => Some(SignalType::Answer),
    "candidate" => Some(SignalType::Candidate),
    "cancel" => Some(SignalType::Cancel),
    _ => None,
  }
}

/// The file a connect names, read out of the ticket without verifying it — the
/// limit applies before we spend anything on a bad ticket.
fn connect_file(req: &Request) -> Option<[u8; 32]> {
  let token = req.params.get("connect_token")?.as_str()?;
  let body: q2q_core::identity::TicketBody = serde_json::from_str(token).ok()?;
  let ih = body.ih?;
  let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, ih).ok()?;
  <[u8; 32]>::try_from(bytes.as_slice()).ok()
}

/// Wall clock in milliseconds. Only the transport needs this — everything that
/// can take time as an argument does.
pub fn now_ms() -> Millis {
  std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map(|d| d.as_millis() as Millis)
    .unwrap_or(0)
}

/// Charge a request from `ip` against its rule.
pub fn charge(limits: &Limiter, req: &Request, ip: IpAddr, now: Millis) -> Verdict {
  let (rule, file) = classify(req);
  let network = NetworkKey::of(ip);
  let key = match file {
    Some(f) => Key::for_file(network, f),
    None => Key::network(network),
  };
  limits.check(rule, &key, 1, now)
}

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;

  fn req(method: &str, params: serde_json::Value) -> Request {
    Request {
      v: 1,
      id: "r".into(),
      method: method.into(),
      params,
    }
  }

  #[test]
  fn each_class_of_request_has_its_own_rule() {
    assert_eq!(classify(&req("punch.join", json!({}))).0, PUNCH_JOIN);
    assert_eq!(classify(&req("punch.exit", json!({}))).0, PUNCH_JOIN);
    assert_eq!(
      classify(&req("punch.heartbeat", json!({}))).0,
      PUNCH_HEARTBEAT
    );
    assert_eq!(
      classify(&req("punch.signal", json!({"type": "connect"}))).0,
      PUNCH_CONNECT
    );
    assert_eq!(
      classify(&req("punch.signal", json!({"type": "candidate"}))).0,
      PUNCH_SIGNAL
    );
  }

  #[test]
  fn a_connect_is_charged_to_the_file_it_names() {
    let token = serde_json::to_string(&serde_json::json!({
      "v": 1, "iss": "x", "pub": "", "iat": 0, "exp": 0, "aud": "punch-1",
      "ih": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [9u8; 32]),
      "sig": ""
    }))
    .unwrap();

    let (rule, file) = classify(&req(
      "punch.signal",
      json!({"type": "connect", "connect_token": token}),
    ));
    assert_eq!(rule, PUNCH_CONNECT);
    assert_eq!(file, Some([9u8; 32]));
  }

  #[test]
  fn one_network_runs_out_of_joins() {
    let limits = limiter_with(1, 120, 20, 120);
    let ip: IpAddr = "10.0.0.1".parse().unwrap();
    let join = req("punch.join", json!({}));

    assert!(charge(&limits, &join, ip, 0).allowed);
    let denied = charge(&limits, &join, ip, 0);
    assert!(!denied.allowed);
    assert!(denied.retry_after_ms.is_some());
  }

  #[test]
  fn another_network_is_unaffected() {
    let limits = limiter_with(1, 120, 20, 120);
    let join = req("punch.join", json!({}));
    assert!(charge(&limits, &join, "10.0.0.1".parse().unwrap(), 0).allowed);
    assert!(
      charge(&limits, &join, "10.0.1.1".parse().unwrap(), 0).allowed,
      "different /24"
    );
  }

  #[test]
  fn a_heartbeat_does_not_draw_from_the_join_bucket() {
    let limits = limiter_with(1, 1, 20, 120);
    let ip: IpAddr = "10.0.0.2".parse().unwrap();
    assert!(charge(&limits, &req("punch.join", json!({})), ip, 0).allowed);
    assert!(charge(&limits, &req("punch.heartbeat", json!({})), ip, 0).allowed);
  }
}
