//! Run explicitly against a disposable Redis instance:
//! Q2Q_REDIS_TEST_HOST=host:port Q2Q_REDIS_TEST_PASSWORD=... \
//!   cargo test -p q2q-track --features redis --test redis_remote -- --ignored

#![cfg(feature = "redis")]

use q2q_core::identity::peer_id_from_public_key;
use q2q_track::redis::RedisState;
use q2q_track::state::{OnlineRecord, PunchAssignment, StateError, TrackState};

#[tokio::test]
#[ignore = "requires Q2Q_REDIS_TEST_HOST and Q2Q_REDIS_TEST_PASSWORD"]
async fn separate_connections_share_sessions_and_holdings() {
  let host = std::env::var("Q2Q_REDIS_TEST_HOST").expect("Redis test host");
  let password = std::env::var("Q2Q_REDIS_TEST_PASSWORD").expect("Redis test password");
  let encoded: String = password
    .as_bytes()
    .iter()
    .map(|byte| format!("%{byte:02X}"))
    .collect();
  let url = format!("redis://:{encoded}@{host}/");
  let prefix = format!(
    "q2q:probe:{}:{}:",
    std::process::id(),
    std::time::SystemTime::now()
      .duration_since(std::time::UNIX_EPOCH)
      .unwrap()
      .as_nanos()
  );

  let client = redis::Client::open(url.as_str()).unwrap();
  let mut conn = client.get_connection_manager().await.unwrap();
  let (now, _): (u64, u64) = redis::cmd("TIME").query_async(&mut conn).await.unwrap();
  let a = RedisState::connect_with_prefix(&url, &prefix)
    .await
    .unwrap();
  let b = RedisState::connect_with_prefix(&url, &prefix)
    .await
    .unwrap();
  let peer = peer_id_from_public_key(&[7; 32]);
  let hash = [5; 32];
  let record = |session_key: [u8; 32], expires_at| OnlineRecord {
    peer_id: peer.clone(),
    public_key: [7; 32],
    session_key,
    last_seq: 0,
    punch: PunchAssignment {
      punch_id: "probe-punch".into(),
      url: String::new(),
      public_key: Vec::new(),
    },
    expires_at,
  };

  a.announce(record([8; 32], now + 120), &[hash], &[], &[hash], 120)
    .await
    .unwrap();
  assert_eq!(
    b.online(&peer, now).await.unwrap().unwrap().session_key,
    [8; 32]
  );
  let candidates = b.query(&hash, 10, None, now).await.unwrap();
  assert_eq!(candidates.len(), 1);
  assert!(candidates[0].complete);

  b.advance_session(&peer, &[8; 32], 1, Some(now + 180), now)
    .await
    .unwrap();
  assert_eq!(a.online(&peer, now).await.unwrap().unwrap().last_seq, 1);
  assert_eq!(
    a.advance_session(&peer, &[8; 32], 1, None, now).await,
    Err(StateError::Replayed(1))
  );

  a.announce(record([9; 32], now + 120), &[hash], &[hash], &[], 120)
    .await
    .unwrap();
  assert_eq!(
    b.advance_session(&peer, &[8; 32], 2, None, now).await,
    Err(StateError::UnknownSession)
  );
  assert_eq!(b.query(&hash, 10, None, now).await.unwrap().len(), 1);

  a.announce(record([9; 32], now + 1), &[hash], &[], &[], 1)
    .await
    .unwrap();
  assert!(b.query(&hash, 10, None, now + 2).await.unwrap().is_empty());
  assert_eq!(b.sweep(now + 2).await.unwrap(), 1);

  let hash_hex: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
  let _: u64 = redis::cmd("DEL")
    .arg(format!("{prefix}online:{}", peer.as_str()))
    .arg(format!("{prefix}file:{hash_hex}:members"))
    .arg(format!("{prefix}file:{hash_hex}:meta"))
    .arg(format!("{prefix}holding-expiry"))
    .query_async(&mut conn)
    .await
    .unwrap();
}
