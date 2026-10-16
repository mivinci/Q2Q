//! Token bucket rate limiting.
//!
//! A fixed window would let an attacker spend the whole quota at the end of one
//! window and again at the start of the next — twice the intended rate in a
//! single instant. The bucket refills continuously, so there is no such edge.
//!
//! Time is passed in by the caller rather than read from a clock: it keeps the
//! tests deterministic and lets a caller with its own notion of time use it.

use crate::network::NetworkKey;
use parking_lot::Mutex;
use std::collections::HashMap;

pub type Millis = u64;

/// What a call is charged against. A network prefix on its own, or a prefix
/// narrowed to one file — the latter matters because a hot file attracts
/// queries from everywhere, and one prefix hammering it should not be able to
/// crowd out everyone else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
  pub network: NetworkKey,
  pub file: Option<[u8; 32]>,
}

impl Key {
  pub fn network(network: NetworkKey) -> Self {
    Key {
      network,
      file: None,
    }
  }

  pub fn for_file(network: NetworkKey, file: [u8; 32]) -> Self {
    Key {
      network,
      file: Some(file),
    }
  }
}

/// One class of request: capacity and how fast it refills.
#[derive(Debug, Clone, Copy)]
pub struct Rule {
  /// Burst size.
  pub capacity: u32,
  /// Tokens added per second.
  pub refill_per_sec: f64,
}

impl Rule {
  pub fn per_minute(capacity: u32, per_minute: u32) -> Self {
    Rule {
      capacity,
      refill_per_sec: per_minute as f64 / 60.0,
    }
  }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
  pub allowed: bool,
  /// How long until one more token is available, for `Retry-After`.
  pub retry_after_ms: Option<Millis>,
  pub remaining: u32,
}

struct Bucket {
  tokens: f64,
  updated_at: Millis,
}

/// Shared limiters live here; a single process can use this directly.
pub struct Limiter {
  rules: HashMap<String, Rule>,
  buckets: Mutex<HashMap<(String, Key), Bucket>>,
  /// Ceiling on tracked keys, so a scan cannot grow the map without bound.
  max_keys: usize,
}

impl Limiter {
  pub fn new(rules: HashMap<String, Rule>) -> Self {
    Limiter {
      rules,
      buckets: Mutex::new(HashMap::new()),
      max_keys: 100_000,
    }
  }

  pub fn with_max_keys(mut self, max_keys: usize) -> Self {
    self.max_keys = max_keys;
    self
  }

  /// Spend `cost` tokens. An unknown rule name is allowed through: a missing
  /// rule is a configuration mistake, and failing open keeps the service up.
  pub fn check(&self, rule: &str, key: &Key, cost: u32, now: Millis) -> Verdict {
    let spec = match self.rules.get(rule) {
      Some(r) => *r,
      None => {
        return Verdict {
          allowed: true,
          retry_after_ms: None,
          remaining: u32::MAX,
        };
      }
    };

    let mut buckets = self.buckets.lock();
    let entry = buckets.entry((rule.to_string(), *key)).or_insert(Bucket {
      tokens: spec.capacity as f64,
      updated_at: now,
    });

    // refill for the elapsed time
    let elapsed_ms = now.saturating_sub(entry.updated_at);
    entry.tokens =
      (entry.tokens + elapsed_ms as f64 / 1000.0 * spec.refill_per_sec).min(spec.capacity as f64);
    entry.updated_at = now;

    let remaining = entry.tokens.floor() as u32;
    if entry.tokens >= cost as f64 {
      entry.tokens -= cost as f64;
      Verdict {
        allowed: true,
        retry_after_ms: None,
        remaining: remaining.saturating_sub(cost),
      }
    } else {
      let missing = cost as f64 - entry.tokens;
      let wait_ms = (missing / spec.refill_per_sec * 1000.0).ceil() as Millis;
      Verdict {
        allowed: false,
        retry_after_ms: Some(wait_ms),
        remaining: 0,
      }
    }
  }

  /// Drop buckets idle for longer than `idle_ms`. Call it on a timer; otherwise
  /// every prefix that ever sends a request stays in the map forever.
  pub fn sweep(&self, now: Millis, idle_ms: Millis) -> usize {
    let mut buckets = self.buckets.lock();
    let before = buckets.len();
    buckets.retain(|_, b| now.saturating_sub(b.updated_at) < idle_ms);
    before - buckets.len()
  }

  pub fn tracked_keys(&self) -> usize {
    self.buckets.lock().len()
  }

  /// Refuse to grow past the ceiling rather than evicting: an attacker able to
  /// evict entries could push out a legitimate peer's bucket.
  pub fn at_capacity(&self) -> bool {
    self.buckets.lock().len() >= self.max_keys
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn limiter(capacity: u32, per_minute: u32) -> Limiter {
    let mut rules = HashMap::new();
    rules.insert("query".to_string(), Rule::per_minute(capacity, per_minute));
    Limiter::new(rules)
  }

  fn v4(o: [u8; 4]) -> NetworkKey {
    NetworkKey::of(std::net::IpAddr::from(o))
  }

  #[test]
  fn allows_up_to_capacity_then_denies() {
    let l = limiter(3, 60);
    let k = Key::network(v4([10, 0, 0, 1]));
    assert!(l.check("query", &k, 1, 0).allowed);
    assert!(l.check("query", &k, 1, 0).allowed);
    assert!(l.check("query", &k, 1, 0).allowed);
    let denied = l.check("query", &k, 1, 0);
    assert!(!denied.allowed);
    assert!(denied.retry_after_ms.is_some());
  }

  #[test]
  fn refills_over_time() {
    // 60 per minute is one token per second
    let l = limiter(2, 60);
    let k = Key::network(v4([10, 0, 0, 2]));
    assert!(l.check("query", &k, 1, 0).allowed);
    assert!(l.check("query", &k, 1, 0).allowed);
    assert!(!l.check("query", &k, 1, 0).allowed);

    // one second later a token is back
    assert!(l.check("query", &k, 1, 1_000).allowed);
  }

  #[test]
  fn separate_networks_get_separate_buckets() {
    let l = limiter(1, 60);
    let a = Key::network(v4([10, 0, 0, 3]));
    let b = Key::network(v4([10, 0, 1, 3]));
    assert!(l.check("query", &a, 1, 0).allowed);
    assert!(
      l.check("query", &b, 1, 0).allowed,
      "different /24, own bucket"
    );
  }

  #[test]
  fn same_network_shares_a_bucket() {
    let l = limiter(1, 60);
    let a = Key::network(v4([10, 0, 0, 4]));
    let b = Key::network(v4([10, 0, 0, 99]));
    assert!(l.check("query", &a, 1, 0).allowed);
    assert!(!l.check("query", &b, 1, 0).allowed);
  }

  #[test]
  fn file_scope_is_independent_of_network_scope() {
    let l = limiter(1, 60);
    let net = v4([10, 0, 0, 5]);
    assert!(l.check("query", &Key::network(net), 1, 0).allowed);
    // the same network, narrowed to a file, is a different bucket
    assert!(
      l.check("query", &Key::for_file(net, [1u8; 32]), 1, 0)
        .allowed
    );
  }

  #[test]
  fn cost_draws_more_than_one_token() {
    let l = limiter(5, 60);
    let k = Key::network(v4([10, 0, 0, 6]));
    assert!(l.check("query", &k, 5, 0).allowed);
    assert!(!l.check("query", &k, 1, 0).allowed);
  }

  #[test]
  fn unknown_rule_fails_open() {
    let l = limiter(1, 60);
    let k = Key::network(v4([10, 0, 0, 7]));
    assert!(l.check("nonexistent", &k, 1, 0).allowed);
  }

  #[test]
  fn retry_after_reflects_the_refill_rate() {
    // 60/min = 1 token/sec, so waiting for one token is ~1000ms
    let l = limiter(1, 60);
    let k = Key::network(v4([10, 0, 0, 8]));
    assert!(l.check("query", &k, 1, 0).allowed);
    let denied = l.check("query", &k, 1, 0);
    assert_eq!(denied.retry_after_ms, Some(1_000));
  }

  #[test]
  fn sweep_drops_idle_buckets() {
    let l = limiter(1, 60);
    let k = Key::network(v4([10, 0, 0, 9]));
    l.check("query", &k, 1, 0);
    assert_eq!(l.tracked_keys(), 1);
    assert_eq!(l.sweep(10_000, 1_000), 1);
    assert_eq!(l.tracked_keys(), 0);
  }

  #[test]
  fn sweep_keeps_recently_used_buckets() {
    let l = limiter(1, 60);
    let k = Key::network(v4([10, 0, 0, 10]));
    l.check("query", &k, 1, 5_000);
    assert_eq!(l.sweep(5_500, 1_000), 0);
  }
}
