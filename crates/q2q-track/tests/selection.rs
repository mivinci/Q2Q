use q2q_core::identity::{PeerId, peer_id_from_public_key};
use q2q_track::{
  selection::{Query, SelectionError},
  state::{MemoryState, OnlineRecord, PunchAssignment, TrackState},
};
use std::collections::HashSet;

fn id(n: u64) -> PeerId {
  peer_id_from_public_key(&key(n))
}
fn key(n: u64) -> [u8; 32] {
  let mut key = [0; 32];
  key[..8].copy_from_slice(&n.to_le_bytes());
  key
}
async fn add(state: &impl TrackState, n: u64, punch: &str, expiry: u64, ttl: u64) {
  state
    .announce(
      OnlineRecord {
        peer_id: id(n),
        public_key: key(n),
        session_key: [7; 32],
        last_seq: 0,
        punch: PunchAssignment {
          punch_id: punch.into(),
          url: "quic://localhost".into(),
          websocket_url: None,
          public_key: vec![0; 32],
        },
        expires_at: expiry,
      },
      &[[1; 32]],
      &[],
      &[],
      ttl,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn filter_before_limit_and_keep_partial_holders() {
  let state = MemoryState::new();
  for n in 0..100 {
    add(
      &state,
      n,
      if n < 90 { "retired" } else { "live" },
      1000,
      600,
    )
    .await;
  }
  add(&state, 101, "live", 400, 600).await; // expired session
  add(&state, 102, "live", 400, 600).await;
  state
    .advance_session(&id(102), &[7; 32], 1, Some(1000), 100)
    .await
    .unwrap(); // heartbeat does not renew holdings
  let punches = HashSet::from(["live".into()]);
  let requester = id(90);
  let query = Query {
    info_hash: &[1; 32],
    requester: &requester,
    limit: 50,
    cursor: None,
    punch_ids: &punches,
    now: 500,
  };
  let selected = query.select(&state, id(22)).await.unwrap();
  assert_eq!(selected.peers.len(), 9);
  assert!(selected.next_cursor.is_none());
  assert!(
    selected
      .peers
      .iter()
      .all(|p| p.peer_id != requester && p.punch_id == "live" && !p.complete)
  );
}

#[tokio::test]
async fn rotated_pages_cover_every_peer_once_and_do_not_change_start() {
  let state = MemoryState::new();
  for n in 0..30 {
    add(&state, n, "live", 1000, 600).await;
  }
  let punches = HashSet::from(["live".into()]);
  let requester = id(0);
  let mut seen = HashSet::new();
  let mut pages = 0;
  let mut cursor: Option<String> = None;
  loop {
    let query = Query {
      info_hash: &[1; 32],
      requester: &requester,
      limit: 3,
      cursor: cursor.as_deref(),
      punch_ids: &punches,
      now: 500,
    };
    let selected = query.select(&state, id(17 + pages)).await.unwrap();
    assert!(selected.peers.len() <= 3);
    for p in selected.peers {
      assert!(
        seen.insert(p.peer_id.to_string()),
        "duplicate across rotated pages"
      );
    }
    pages += 1;
    cursor = selected.next_cursor;
    if cursor.is_none() {
      break;
    }
    assert!(pages < 12);
  }
  assert_eq!(seen.len(), 29);
}

#[tokio::test]
async fn fresh_start_changes_selection_and_legacy_cursor_remains_lexical() {
  let state = MemoryState::new();
  for n in 0..10 {
    add(&state, n, "live", 1000, 600).await;
  }
  let punches = HashSet::from(["live".into()]);
  let requester = id(99);
  let mut query = Query {
    info_hash: &[1; 32],
    requester: &requester,
    limit: 2,
    cursor: None,
    punch_ids: &punches,
    now: 500,
  };
  let a = query.select(&state, id(1)).await.unwrap();
  let b = query.select(&state, id(2)).await.unwrap();
  assert_ne!(a.peers[0].peer_id, b.peers[0].peer_id);
  let legacy = id(4).to_string();
  query.cursor = Some(&legacy);
  let page = query.select(&state, id(8)).await.unwrap();
  assert!(
    page
      .peers
      .iter()
      .all(|p| p.peer_id.as_str() > legacy.as_str())
  );
}

#[tokio::test]
async fn scan_budget_can_return_empty_page_with_a_continuation() {
  let state = MemoryState::new();
  for n in 0..600 {
    add(&state, n, "retired", 1000, 600).await;
  }
  let punches = HashSet::from(["live".into()]);
  let requester = id(999);
  let mut query = Query {
    info_hash: &[1; 32],
    requester: &requester,
    limit: 5,
    cursor: None,
    punch_ids: &punches,
    now: 500,
  };
  let first = query.select(&state, id(7)).await.unwrap();
  assert!(first.peers.is_empty());
  let cursor = first.next_cursor.unwrap();
  query.cursor = Some(&cursor);
  let last = query.select(&state, id(8)).await.unwrap();
  assert!(last.peers.is_empty());
  assert!(last.next_cursor.is_none());
}

#[tokio::test]
async fn cursors_are_bounded_versioned_and_bound_to_file_and_requester() {
  let state = MemoryState::new();
  for n in 0..4 {
    add(&state, n, "live", 1000, 600).await;
  }
  let punches = HashSet::from(["live".into()]);
  let requester = id(99);
  let mut query = Query {
    info_hash: &[1; 32],
    requester: &requester,
    limit: 1,
    cursor: None,
    punch_ids: &punches,
    now: 500,
  };
  let cursor = query
    .select(&state, id(1))
    .await
    .unwrap()
    .next_cursor
    .unwrap();
  for raw in [
    "bad".to_owned(),
    "v2.abc".into(),
    "v1.e30".into(),
    "x".repeat(1025),
  ] {
    let query = Query {
      info_hash: &[1; 32],
      requester: &requester,
      limit: 1,
      cursor: Some(&raw),
      punch_ids: &punches,
      now: 500,
    };
    assert!(matches!(
      query.select(&state, id(1)).await,
      Err(SelectionError::InvalidCursor)
    ));
  }
  query.cursor = Some(&cursor);
  query.info_hash = &[2; 32];
  assert!(matches!(
    query.select(&state, id(1)).await,
    Err(SelectionError::InvalidCursor)
  ));
  query.info_hash = &[1; 32];
  let other = id(98);
  query.requester = &other;
  assert!(matches!(
    query.select(&state, id(1)).await,
    Err(SelectionError::InvalidCursor)
  ));
  query.cursor = None;
  query.limit = 0;
  assert!(matches!(
    query.select(&state, id(1)).await,
    Err(SelectionError::InvalidLimit)
  ));
}

#[cfg(feature = "redis")]
#[tokio::test]
#[ignore = "requires Q2Q_REDIS_TEST_HOST and Q2Q_REDIS_TEST_PASSWORD"]
async fn redis_and_memory_produce_identical_rotating_pages() {
  let host = std::env::var("Q2Q_REDIS_TEST_HOST").unwrap();
  let password = std::env::var("Q2Q_REDIS_TEST_PASSWORD").unwrap();
  let encoded: String = password
    .as_bytes()
    .iter()
    .map(|byte| format!("%{byte:02X}"))
    .collect();
  let url = format!("redis://:{encoded}@{host}/");
  let prefix = format!(
    "q2q:selection:{}:{}:",
    std::process::id(),
    std::time::SystemTime::now()
      .duration_since(std::time::UNIX_EPOCH)
      .unwrap()
      .as_nanos()
  );
  let memory = MemoryState::new();
  let shared = q2q_track::redis::RedisState::connect_with_prefix(&url, &prefix)
    .await
    .unwrap();
  let now = q2q_track::state::now();
  for n in 0..80 {
    let punch = if n < 50 { "retired" } else { "live" };
    add(&memory, n, punch, now + 600, 600).await;
    add(&shared, n, punch, now + 600, 600).await;
  }
  let requester = id(55);
  let punches = HashSet::from(["live".into()]);
  let mut cursor: Option<String> = None;
  let mut count = 0;
  loop {
    let query = Query {
      info_hash: &[1; 32],
      requester: &requester,
      limit: 7,
      cursor: cursor.as_deref(),
      punch_ids: &punches,
      now,
    };
    let a = query.select(&memory, id(22)).await.unwrap();
    let b = query.select(&shared, id(22)).await.unwrap();
    let ids = |peers: &[q2q_track::state::Candidate]| {
      peers
        .iter()
        .map(|peer| peer.peer_id.to_string())
        .collect::<Vec<_>>()
    };
    assert_eq!(ids(&a.peers), ids(&b.peers));
    assert_eq!(a.next_cursor, b.next_cursor);
    count += a.peers.len();
    cursor = a.next_cursor;
    if cursor.is_none() {
      break;
    }
  }
  assert_eq!(count, 29);
  let mut conn = redis::Client::open(url)
    .unwrap()
    .get_connection_manager()
    .await
    .unwrap();
  let mut clean = redis::cmd("DEL");
  for n in 0..80 {
    clean.arg(format!("{prefix}online:{}", id(n)));
  }
  clean
    .arg(format!("{prefix}file:{}:members", "01".repeat(32)))
    .arg(format!("{prefix}file:{}:meta", "01".repeat(32)))
    .arg(format!("{prefix}holding-expiry"));
  let _: u64 = clean.query_async(&mut conn).await.unwrap();
}
