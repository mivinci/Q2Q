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

/// Per network prefix, per minute.
#[derive(Debug, Clone, Copy)]
pub struct Rates {
  /// Establishing a binding verifies a signature, so it is the expensive one.
  pub join: u32,
  /// Frequent, but cheap to reject: a forged one fails its MAC first.
  pub heartbeat: u32,
  /// The only call that lands on somebody else.
  pub connect: u32,
  pub signal: u32,
}

impl Default for Rates {
  fn default() -> Self {
    Rates {
      join: 10,
      heartbeat: 120,
      connect: 20,
      signal: 120,
    }
  }
}

/// Buckets are per instance, so each one enforces only its share. Without this,
/// N instances each holding the full quota would allow N times the rate we
/// intended — which is exactly what an attacker gets by spreading requests
/// across every instance (they are all directly reachable).
fn share(per_minute: u32, cluster_size: u32) -> u32 {
  (per_minute / cluster_size.max(1)).max(1)
}

pub fn limiter(rates: Rates, cluster_size: u32) -> Limiter {
  let mut rules = HashMap::new();
  for (name, per_minute) in [
    (PUNCH_JOIN, rates.join),
    (PUNCH_HEARTBEAT, rates.heartbeat),
    (PUNCH_CONNECT, rates.connect),
    (PUNCH_SIGNAL, rates.signal),
  ] {
    let share = share(per_minute, cluster_size);
    rules.insert(name.to_string(), Rule::per_minute(share, share));
  }
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
  let body: q2q_primitives::identity::TicketBody = serde_json::from_str(token).ok()?;
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
      "v": 1, "iss": "x", "pub": "", "iat": 0, "exp": 0, "aud": "punch:punch-1",
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

  fn rates(join: u32, heartbeat: u32, connect: u32, signal: u32) -> Rates {
    Rates {
      join,
      heartbeat,
      connect,
      signal,
    }
  }

  #[test]
  fn many_instances_each_enforce_a_share() {
    // three instances, no shared bucket: one of them gets a third, so that
    // spreading requests across all three still adds up to the intended rate
    let limits = limiter(
      Rates {
        join: 9,
        ..Rates::default()
      },
      3,
    );
    let ip: IpAddr = "10.0.0.9".parse().unwrap();
    let join = req("punch.join", json!({}));
    for _ in 0..3 {
      assert!(charge(&limits, &join, ip, 0).allowed);
    }
    assert!(!charge(&limits, &join, ip, 0).allowed);
  }

  #[test]
  fn one_network_runs_out_of_joins() {
    let limits = limiter(rates(1, 120, 20, 120), 1);
    let ip: IpAddr = "10.0.0.1".parse().unwrap();
    let join = req("punch.join", json!({}));

    assert!(charge(&limits, &join, ip, 0).allowed);
    let denied = charge(&limits, &join, ip, 0);
    assert!(!denied.allowed);
    assert!(denied.retry_after_ms.is_some());
  }

  #[test]
  fn another_network_is_unaffected() {
    let limits = limiter(rates(1, 120, 20, 120), 1);
    let join = req("punch.join", json!({}));
    assert!(charge(&limits, &join, "10.0.0.1".parse().unwrap(), 0).allowed);
    assert!(
      charge(&limits, &join, "10.0.1.1".parse().unwrap(), 0).allowed,
      "different /24"
    );
  }

  #[test]
  fn a_heartbeat_does_not_draw_from_the_join_bucket() {
    let limits = limiter(rates(1, 1, 20, 120), 1);
    let ip: IpAddr = "10.0.0.2".parse().unwrap();
    assert!(charge(&limits, &req("punch.join", json!({})), ip, 0).allowed);
    assert!(charge(&limits, &req("punch.heartbeat", json!({})), ip, 0).allowed);
  }
}
