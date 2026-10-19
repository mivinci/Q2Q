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
fn punch_assignment_detects_websocket_endpoint_changes() {
  let current = q2q_track::wire::PunchWire {
    punch_id: "punch-1".into(),
    url: "quic://punch.example:4433".into(),
    websocket_url: Some("wss://punch.example/punch".into()),
    public_key: "key".into(),
  };
  let mut next = current.clone();
  next.websocket_url = Some("wss://new-punch.example/punch".into());
  assert!(punch_assignment_changed(&current, &next));

  next.websocket_url = current.websocket_url.clone();
  assert!(!punch_assignment_changed(&current, &next));
}
