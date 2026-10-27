use super::*;

#[test]
fn query_options_preserve_defaults_and_encode_opaque_cursors() {
  let mut url = Url::parse("https://track.example/files/hash/peers").unwrap();
  QueryOptions::default().apply(&mut url).unwrap();
  assert!(url.query().is_none());
  let cursor = "v1.opaque/+?= &中文";
  QueryOptions {
    limit: Some(8),
    cursor: Some(cursor.into()),
  }
  .apply(&mut url)
  .unwrap();
  let pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
  assert_eq!(pairs["limit"], "8");
  assert_eq!(pairs["cursor"], cursor);
}
#[test]
fn query_options_reject_invalid_limits_and_oversized_cursors() {
  for limit in [0, 51, usize::MAX] {
    let mut url = Url::parse("https://track.example/").unwrap();
    assert!(
      QueryOptions {
        limit: Some(limit),
        cursor: None
      }
      .apply(&mut url)
      .is_err()
    );
  }
  for limit in [1, 50] {
    let mut url = Url::parse("https://track.example/").unwrap();
    QueryOptions {
      limit: Some(limit),
      cursor: None,
    }
    .apply(&mut url)
    .unwrap();
  }
  let mut url = Url::parse("https://track.example/").unwrap();
  assert!(
    QueryOptions {
      limit: None,
      cursor: Some("a".repeat(1025))
    }
    .apply(&mut url)
    .is_err()
  );
}

#[test]
fn punch_assignment_detects_endpoint_changes() {
  let current = q2q_track::wire::PunchWire {
    punch_id: "punch-1".into(),
    url: "quic://punch.example:4433".into(),
    public_key: "key".into(),
  };
  let mut next = current.clone();
  next.url = "quic://new-punch.example:4433".into();
  assert!(punch_assignment_changed(&current, &next));

  next.url = current.url.clone();
  assert!(!punch_assignment_changed(&current, &next));
}

fn lease_hash(index: u16) -> [u8; 32] {
  let mut hash = [0; 32];
  hash[..2].copy_from_slice(&index.to_be_bytes());
  hash
}

#[test]
fn staggered_publication_timers_share_one_batch_without_upgrading_partial_files() {
  let mut holdings = (0..32)
    .map(|i| {
      (
        lease_hash(i),
        HoldingLease {
          expires: 300 + u64::from(i),
          complete: i % 2 == 0,
        },
      )
    })
    .collect::<BTreeMap<_, _>>();
  let mut requests = 0;
  // Every old per-file timer fires, staggered across the next 16 seconds.
  for i in 0..32 {
    let hash = lease_hash(i);
    let now = 150 + u64::from(i) / 2;
    if holdings[&hash].due(now) {
      requests += 1;
      for member in renewal_batch(&holdings, hash) {
        holdings.get_mut(&member).unwrap().expires = now + SESSION_TTL;
      }
    }
  }
  assert_eq!(requests, 1);
  assert!(holdings.values().all(|lease| lease.expires == 450));
  for i in 0..32 {
    assert_eq!(holdings[&lease_hash(i)].complete, i % 2 == 0);
  }
}

#[test]
fn renewal_prioritizes_oldest_leases_and_never_includes_withdrawn_files() {
  let mut holdings = (0..257)
    .map(|i| {
      (
        lease_hash(i),
        HoldingLease {
          expires: 300,
          complete: true,
        },
      )
    })
    .collect::<BTreeMap<_, _>>();
  holdings.remove(&lease_hash(1));
  let first = renewal_batch(&holdings, lease_hash(256));
  assert_eq!(first.len(), 128);
  assert_eq!(first[0], lease_hash(256));
  assert!(!first.contains(&lease_hash(1)));
  for hash in &first {
    holdings.get_mut(hash).unwrap().expires = 450;
  }
  let next = renewal_batch(&holdings, lease_hash(255));
  assert!(next.iter().all(|hash| holdings[hash].expires == 300));
  assert_eq!(next.len(), 128);
}

#[test]
fn renewal_is_due_at_half_life_and_after_expiry() {
  let lease = HoldingLease {
    expires: 300,
    complete: false,
  };
  assert!(!lease.due(149));
  assert!(lease.due(150));
  assert!(lease.due(400));
}

#[tokio::test]
#[ignore = "local Workers/Punch fixture: npm run test:sdk --workspace @q2q/track"]
async fn batch_renewal_preserves_remote_partial_state_and_serializes_withdrawal() {
  let root = tempfile::tempdir().unwrap();
  let identity = Identity::load_or_create(&root.path().join("identity")).unwrap();
  let url = Url::parse(&std::env::var("Q2Q_TEST_TRACK_URL").unwrap()).unwrap();
  let session = Arc::new(
    ControlSession::connect(&identity, &url, false)
      .await
      .unwrap(),
  );
  let hashes = (0..32).map(lease_hash).collect::<Vec<_>>();
  session
    .update_holdings(&hashes, &[], &hashes[..16])
    .await
    .unwrap();
  // Force the local scheduler's due boundary without sleeping 150 seconds;
  // wire tickets, server clocks and the remote lease stay real.
  {
    let mut auth = session.track_auth.lock().await;
    for lease in auth.holdings.values_mut() {
      lease.expires = q2q_track::state::now();
    }
  }
  let renewed =
    futures_util::future::join_all(hashes.iter().map(|hash| session.renew_holdings(*hash))).await;
  let expiry = *renewed[0].as_ref().unwrap();
  assert!(
    renewed
      .iter()
      .all(|result| result.as_ref().is_ok_and(|value| *value == expiry))
  );
  let key = session.track_auth.lock().await.key;
  for hash in &hashes {
    assert_eq!(session.renew_holdings(*hash).await.unwrap(), expiry);
    assert_eq!(
      session.track_auth.lock().await.key,
      key,
      "stale timer rotated session"
    );
  }
  session.heartbeat().await.unwrap();
  // A distinct requester sees both complete and partial members after renewal.
  let reader_identity = Identity::load_or_create(&root.path().join("reader")).unwrap();
  let reader = Arc::new(
    ControlSession::connect(&reader_identity, &url, false)
      .await
      .unwrap(),
  );
  for i in [0, 31] {
    let page = reader.query(&hashes[i]).await.unwrap();
    assert_eq!(page.peers.len(), 1);
    assert_eq!(page.peers[0].complete, i < 16);
  }
  session
    .track_auth
    .lock()
    .await
    .holdings
    .get_mut(&hashes[31])
    .unwrap()
    .expires = 1;
  let (removed, renewed) = tokio::join!(
    session.update_files(&[], &hashes[..1]),
    session.renew_holdings(hashes[31])
  );
  removed.unwrap();
  renewed.unwrap();
  assert!(session.renew_holdings(hashes[0]).await.is_err());
  assert!(reader.query(&hashes[0]).await.unwrap().peers.is_empty());
  // Drain the real default prefix bucket: a rejected renewal must preserve
  // the old key and lease, and a rejected withdrawal must stop local renewal.
  loop {
    match session.update_files(&[], &[]).await {
      Ok(_) => {}
      Err(ControlError::Remote { status: 429, .. }) => break,
      Err(error) => panic!("unexpected failure: {error}"),
    }
  }
  let old_key = {
    let mut auth = session.track_auth.lock().await;
    auth.holdings.get_mut(&hashes[31]).unwrap().expires = 1;
    auth.key
  };
  assert!(session.renew_holdings(hashes[31]).await.is_err());
  {
    let auth = session.track_auth.lock().await;
    assert_eq!(auth.key, old_key);
    assert_eq!(auth.holdings[&hashes[31]].expires, 1);
  }
  assert!(session.update_files(&[], &hashes[1..]).await.is_err());
  assert!(session.track_auth.lock().await.holdings.is_empty());
  reader.close().await.unwrap();
  session.close().await.unwrap();
}
