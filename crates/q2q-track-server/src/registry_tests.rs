use super::*;

fn node(id: &str, assignable: bool, expires_at: Instant) -> LeasedNode {
  LeasedNode {
    assignment: PunchAssignment {
      punch_id: id.into(),
      url: format!("quic://{id}.example:4433"),
      public_key: vec![7; 32],
    },
    assignable,
    expires_at,
  }
}
fn directory(nodes: Vec<LeasedNode>) -> RegistryPunchDirectory {
  RegistryPunchDirectory {
    client: Client::new(),
    endpoint: Url::parse("https://registry.example/v1/discovery/punches").unwrap(),
    cache: RwLock::new(Some(Cache {
      nodes,
      expires_at: Instant::now() + Duration::from_secs(90),
    })),
    identities: RwLock::new(HashMap::new()),
    token_expires_at: 9_007_199_254_740_991,
    node_id: "test-track".into(),
    report_endpoint: Url::parse("https://registry.example/v1/nodes/self").unwrap(),
    report_enabled: true,
    region: Some("hk".into()),
    accepting: true,
  }
}

#[test]
fn assignments_reach_both_nodes_and_ignore_discovery_order() {
  let until = Instant::now() + Duration::from_secs(60);
  let first = directory(vec![node("hk", true, until), node("cd", true, until)]);
  let replica = directory(vec![node("cd", true, until), node("hk", true, until)]);
  let mut counts = HashMap::new();
  for id in 0..512 {
    let peer = PeerId::unchecked(format!("peer-{id}"));
    let selected = first.assign(&peer).unwrap().unwrap();
    assert_eq!(Some(selected.clone()), replica.assign(&peer).unwrap());
    *counts.entry(selected.punch_id).or_insert(0usize) += 1;
  }
  assert_eq!(counts.len(), 2);
  assert!(
    counts.values().all(|&count| (128..384).contains(&count)),
    "{counts:?}"
  );
}

#[test]
fn removing_a_node_only_changes_its_own_assignments() {
  let until = Instant::now() + Duration::from_secs(60);
  let before = directory(vec![
    node("hk", true, until),
    node("cd", true, until),
    node("sg", true, until),
  ]);
  let after = directory(vec![node("sg", true, until), node("hk", true, until)]);
  let mut moved = 0;
  for id in 0..512 {
    let peer = PeerId::unchecked(format!("peer-{id}"));
    let previous = before.assign(&peer).unwrap().unwrap();
    let next = after.assign(&peer).unwrap().unwrap();
    if previous.punch_id == "cd" {
      moved += 1;
    } else {
      assert_eq!(previous, next);
    }
    assert_ne!(next.punch_id, "cd");
  }
  assert!(moved > 0);
}

#[test]
fn draining_nodes_remain_queryable_but_expired_nodes_do_not() {
  let now = Instant::now();
  let directory = directory(vec![
    node("draining", false, now + Duration::from_secs(60)),
    node("expired", true, now - Duration::from_secs(1)),
  ]);
  let peer = PeerId::unchecked("peer");
  assert!(directory.assign(&peer).unwrap().is_none());
  let snapshot = directory.snapshot().unwrap();
  assert!(snapshot.nodes.contains_key("draining"));
  assert!(!snapshot.nodes.contains_key("expired"));
  directory
    .cache
    .write()
    .unwrap()
    .as_mut()
    .unwrap()
    .expires_at = now;
  assert!(directory.assign(&peer).is_err());
  assert!(directory.snapshot().is_err());
}

use axum::{Router, extract::State, response::IntoResponse, routing::get};
use serde_json::{Value, json};
use std::{collections::VecDeque, sync::Mutex};

type Replies = Arc<Mutex<VecDeque<(u16, Value)>>>;

struct MockRegistry {
  directory: Arc<RegistryPunchDirectory>,
  replies: Replies,
  requests: Arc<Mutex<Vec<String>>>,
  task: tokio::task::JoinHandle<()>,
}

impl Drop for MockRegistry {
  fn drop(&mut self) {
    self.task.abort();
  }
}

impl MockRegistry {
  async fn new() -> Self {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let replies: Replies = Arc::new(Mutex::new(VecDeque::from([(503, json!({}))])));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
      .route(
        "/v1/discovery/punches",
        get(
          |State((replies, requests)): State<(Replies, Arc<Mutex<Vec<String>>>)>,
           uri: axum::http::Uri| async move {
            requests.lock().unwrap().push(uri.to_string());
            let mut queue = replies.lock().unwrap();
            let (status, body) = if queue.len() > 1 {
              queue.pop_front().unwrap()
            } else {
              queue.front().unwrap().clone()
            };
            (
              axum::http::StatusCode::from_u16(status).unwrap(),
              axum::Json(body),
            )
              .into_response()
          },
        ),
      )
      .with_state((replies.clone(), requests.clone()));
    let mut directory = directory(Vec::new());
    directory.client = Client::builder()
      .no_proxy()
      .timeout(Duration::from_secs(2))
      .build()
      .unwrap();
    directory.endpoint = Url::parse(&format!(
      "http://{}/v1/discovery/punches",
      listener.local_addr().unwrap()
    ))
    .unwrap();
    directory.cache = RwLock::new(None);
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Self {
      directory: Arc::new(directory),
      replies,
      requests,
      task,
    }
  }

  fn respond(&self, replies: Vec<(u16, Value)>) {
    assert!(!replies.is_empty());
    *self.replies.lock().unwrap() = replies.into();
  }
}

fn wire_node(id: &str, version: u64) -> Value {
  json!({"punch_id": id, "url": format!("quic://v{version}.example:4433"),
    "public_key": STANDARD.encode([7; 32]), "identity_version": version,
    "accepting_new_connections": true, "last_seen": 100, "expires_at": 190, "metrics": null})
}

fn page(nodes: Vec<Value>, cursor: Option<&str>) -> Value {
  json!({"server_time": 100, "nodes": nodes, "next_cursor": cursor})
}

#[tokio::test]
async fn http_failure_preserves_original_deadline_and_recovery_replaces_snapshot() {
  let mock = MockRegistry::new().await;
  assert!(mock.directory.snapshot().is_err());
  mock.respond(vec![(200, page(vec![wire_node("old", 1)], None))]);
  assert!(mock.directory.refresh().await.is_ok());
  let deadline = mock
    .directory
    .cache
    .read()
    .unwrap()
    .as_ref()
    .unwrap()
    .expires_at;
  for status in [503, 429, 401, 403] {
    mock.respond(vec![(status, json!({"detail": "private upstream detail"}))]);
    assert!(mock.directory.refresh().await.is_err());
    assert!(mock.directory.get("old").unwrap().is_some());
    assert_eq!(
      mock
        .directory
        .cache
        .read()
        .unwrap()
        .as_ref()
        .unwrap()
        .expires_at,
      deadline
    );
  }
  // Expiration is checked at read time, independently of another refresh.
  mock
    .directory
    .cache
    .write()
    .unwrap()
    .as_mut()
    .unwrap()
    .expires_at = Instant::now();
  assert!(mock.directory.snapshot().is_err());
  mock.respond(vec![(200, page(vec![wire_node("recovered", 1)], None))]);
  assert!(mock.directory.refresh().await.is_ok());
  assert!(mock.directory.get("old").unwrap().is_none());
  assert!(mock.directory.get("recovered").unwrap().is_some());
  mock.respond(vec![(200, page(vec![], None))]);
  assert!(mock.directory.refresh().await.is_ok());
  assert!(mock.directory.snapshot().unwrap().nodes.is_empty());
}

#[tokio::test]
async fn pagination_failure_never_publishes_partial_or_duplicate_nodes() {
  let mock = MockRegistry::new().await;
  mock.respond(vec![(200, page(vec![wire_node("old", 1)], None))]);
  assert!(mock.directory.refresh().await.is_ok());
  for second in [
    (503, json!({})),
    (200, page(vec![wire_node("first", 1)], None)),
    (200, page(vec![wire_node("second", 1)], Some("first"))),
  ] {
    mock.respond(vec![
      (200, page(vec![wire_node("first", 1)], Some("first"))),
      second,
    ]);
    assert!(mock.directory.refresh().await.is_err());
    assert!(mock.directory.get("old").unwrap().is_some());
    assert!(mock.directory.get("first").unwrap().is_none());
  }
  mock.respond(vec![
    (200, page(vec![wire_node("first", 1)], Some("first"))),
    (200, page(vec![wire_node("second", 1)], None)),
  ]);
  assert!(mock.directory.refresh().await.is_ok());
  assert_eq!(mock.directory.snapshot().unwrap().nodes.len(), 2);
  assert!(
    mock
      .requests
      .lock()
      .unwrap()
      .iter()
      .any(|path| path.contains("after=first"))
  );
}

#[tokio::test]
async fn discovery_rejects_identity_rollback_even_after_node_removal() {
  let mock = MockRegistry::new().await;
  mock.respond(vec![(200, page(vec![wire_node("a", 2)], None))]);
  assert!(mock.directory.refresh().await.is_ok());
  mock.respond(vec![(200, page(vec![], None))]);
  assert!(mock.directory.refresh().await.is_ok());
  assert!(mock.directory.get("a").unwrap().is_none());
  mock.respond(vec![(200, page(vec![wire_node("a", 1)], None))]);
  assert!(matches!(
    mock.directory.refresh().await,
    Err(FetchError::Invalid)
  ));
  let mut conflict = wire_node("a", 2);
  conflict["public_key"] = json!(STANDARD.encode([8; 32]));
  mock.respond(vec![(200, page(vec![conflict], None))]);
  assert!(matches!(
    mock.directory.refresh().await,
    Err(FetchError::Invalid)
  ));
  mock.respond(vec![(200, page(vec![wire_node("a", 3)], None))]);
  assert!(mock.directory.refresh().await.is_ok());
  assert_eq!(
    mock.directory.get("a").unwrap().unwrap().url,
    "quic://v3.example:4433"
  );
}

#[tokio::test]
async fn discovery_loop_recovers_after_outage_and_cancels_on_shutdown() {
  let mock = MockRegistry::new().await;
  mock.respond(vec![
    (503, json!({})),
    (200, page(vec![wire_node("a", 1)], None)),
  ]);
  let (stop, rx) = watch::channel(false);
  let task = tokio::spawn(mock.directory.clone().run_discovery(rx));
  let recovered = tokio::time::timeout(Duration::from_secs(4), async {
    loop {
      if mock.directory.snapshot().is_ok() {
        break;
      }
      tokio::time::sleep(Duration::from_millis(10)).await;
    }
  })
  .await;
  stop.send(true).unwrap();
  tokio::time::timeout(Duration::from_secs(1), task)
    .await
    .unwrap()
    .unwrap();
  assert!(recovered.is_ok());
  assert!(mock.requests.lock().unwrap().len() >= 2);
}

#[tokio::test]
async fn discovery_auth_rejection_stops_requests_until_restart() {
  let mock = MockRegistry::new().await;
  mock.respond(vec![
    (403, json!({})),
    (200, page(vec![wire_node("a", 1)], None)),
  ]);
  let (stop, rx) = watch::channel(false);
  let task = tokio::spawn(mock.directory.clone().run_discovery(rx));
  tokio::time::timeout(Duration::from_secs(1), async {
    while mock.requests.lock().unwrap().is_empty() {
      tokio::task::yield_now().await;
    }
  })
  .await
  .unwrap();
  tokio::time::sleep(Duration::from_millis(1300)).await;
  assert_eq!(mock.requests.lock().unwrap().len(), 1);
  assert!(mock.directory.snapshot().is_err());
  stop.send(true).unwrap();
  tokio::time::timeout(Duration::from_secs(1), task)
    .await
    .unwrap()
    .unwrap();
}

#[tokio::test]
async fn failed_refresh_does_not_advance_identity_history() {
  let mock = MockRegistry::new().await;
  mock.respond(vec![(200, page(vec![wire_node("a", 2)], None))]);
  assert!(mock.directory.refresh().await.is_ok());
  mock.respond(vec![
    (200, page(vec![wire_node("a", 4)], Some("a"))),
    (503, json!({})),
  ]);
  assert!(mock.directory.refresh().await.is_err());
  mock.respond(vec![(200, page(vec![wire_node("a", 3)], None))]);
  assert!(mock.directory.refresh().await.is_ok());
  assert_eq!(
    mock.directory.get("a").unwrap().unwrap().url,
    "quic://v3.example:4433"
  );
}

#[tokio::test]
async fn shutdown_cancels_stalled_discovery_request() {
  let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
  let mut directory = directory(Vec::new());
  directory.client = Client::builder()
    .no_proxy()
    .timeout(Duration::from_secs(10))
    .build()
    .unwrap();
  directory.endpoint = Url::parse(&format!(
    "http://{}/v1/discovery/punches",
    listener.local_addr().unwrap()
  ))
  .unwrap();
  let (stop, rx) = watch::channel(false);
  let task = tokio::spawn(Arc::new(directory).run_discovery(rx));
  let (_stalled, _) = tokio::time::timeout(Duration::from_secs(1), listener.accept())
    .await
    .unwrap()
    .unwrap();
  stop.send(true).unwrap();
  tokio::time::timeout(Duration::from_secs(1), task)
    .await
    .unwrap()
    .unwrap();
}
