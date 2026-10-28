mod support;
use q2q_http::{Error, Executor};
use std::time::Duration;
use support::{Reply, Server};

#[tokio::test]
async fn range_validation_and_bounded_unknown_length_metadata() {
  let server = Server::start(|r| {
    let mut reply = Reply::range(r, b"abcdef");
    match r.path.as_str() {
      "/range" => {}
      "/full" => return Reply::body(b"abcdef"),
      "/bad-range" => reply.headers = vec![("Content-Range".into(), "bytes 1-3/6".into())],
      "/short" => {
        reply.body.pop();
        reply.length = None;
      }
      "/long" => {
        reply.body.push(0);
        reply.length = None;
      }
      "/compressed" => reply
        .headers
        .push(("Content-Encoding".into(), "gzip".into())),
      "/status" => reply.status = 416,
      "/no-length" => reply.length = None,
      _ => panic!(),
    }
    reply
  })
  .await;
  let http = Executor::builder().build().unwrap();
  for path in ["bad-range", "short", "long", "compressed", "status", "full"] {
    let result = async {
      http
        .get_range(format!("{}/{path}", server.url), 0..3, 6)
        .send()
        .await?
        .collect_exact(3)
        .await
    }
    .await;
    assert!(result.is_err(), "{path}");
    assert_eq!(http.active_requests(), 0);
  }
  assert_eq!(
    &http
      .get_range(format!("{}/no-length", server.url), 0..3, 6)
      .send()
      .await
      .unwrap()
      .collect_exact(3)
      .await
      .unwrap()[..],
    b"abc"
  );
  assert_eq!(
    &http
      .get_range(format!("{}/full", server.url), 0..6, 6)
      .send()
      .await
      .unwrap()
      .collect_exact(6)
      .await
      .unwrap()[..],
    b"abcdef"
  );
  assert!(
    http
      .get(format!("{}/full", server.url))
      .max_body_size(3)
      .send()
      .await
      .is_err()
  );
  let meta = Server::start(|_| {
    let mut r = Reply::body(b"metadata");
    r.length = None;
    r
  })
  .await;
  assert!(
    http
      .get(&meta.url)
      .send()
      .await
      .unwrap()
      .collect_bounded(3)
      .await
      .is_err()
  );
  assert_eq!(
    &http
      .get(&meta.url)
      .send()
      .await
      .unwrap()
      .collect_bounded(8)
      .await
      .unwrap()[..],
    b"metadata"
  );
}
#[tokio::test]
async fn response_owns_permits_until_drop_or_checked_eof() {
  let server = Server::start(|_| Reply::body(b"body")).await;
  let http = Executor::builder().max_requests(1).build().unwrap();
  let response = http.get(&server.url).send().await.unwrap();
  assert_eq!(http.active_requests(), 1);
  let second = http.get(&server.url).send();
  tokio::pin!(second);
  assert!(
    tokio::time::timeout(Duration::from_millis(20), &mut second)
      .await
      .is_err()
  );
  assert_eq!(server.requests.lock().unwrap().len(), 1);
  drop(response);
  let response = second.await.unwrap();
  assert_eq!(response.collect_exact(4).await.unwrap().len(), 4);
  assert_eq!(http.active_requests(), 0);
}
#[tokio::test]
async fn deadline_covers_body_and_failure_releases_budget() {
  let server = Server::start(|_| {
    let mut r = Reply::body(b"slow");
    r.delay = Duration::from_millis(100);
    r
  })
  .await;
  let http = Executor::builder()
    .timeout(Duration::from_millis(30))
    .build()
    .unwrap();
  let mut response = http.get(&server.url).send().await.unwrap();
  assert!(matches!(response.chunk().await, Err(Error::Timeout)));
  assert_eq!(http.active_requests(), 0);
  assert!(response.chunk().await.is_err());
}
#[tokio::test]
async fn redirects_reapply_host_budgets_preserve_range_and_hide_grants() {
  let dest = Server::start(|r| Reply::range(r, b"abcdef")).await;
  let target = dest.url.clone();
  let entry = Server::start(move |r| {
    if r.path == "/normal" {
      return Reply::body(b"ok");
    }
    let mut res = Reply::body(b"");
    res.status = 307;
    res.header("Location", &target)
  })
  .await;
  let http = Executor::builder()
    .max_requests(2)
    .max_requests_per_host(1)
    .build()
    .unwrap();
  let hold = http.get(&dest.url).send().await.unwrap();
  let redir = http
    .get_range(format!("{}/grant?secret=private", entry.url), 0..3, 6)
    .send();
  tokio::pin!(redir);
  assert!(
    tokio::time::timeout(Duration::from_millis(30), &mut redir)
      .await
      .is_err()
  );
  // A blocked destination must not occupy the global slot or the entry host.
  assert_eq!(
    http
      .get(format!("{}/normal", entry.url))
      .send()
      .await
      .unwrap()
      .collect_exact(2)
      .await
      .unwrap()
      .len(),
    2
  );
  drop(hold);
  assert_eq!(
    redir.await.unwrap().collect_exact(3).await.unwrap().len(),
    3
  );
  {
    let requests = dest.requests.lock().unwrap();
    assert_eq!(requests.last().unwrap().range.as_deref(), Some("bytes=0-2"));
    assert!(requests.iter().all(|r| r.referer.is_none() && r.identity));
  }
  let e = http
    .get("http://127.0.0.1:1/file?secret=private")
    .send()
    .await
    .err()
    .unwrap();
  assert!(!format!("{e:?} {e}").contains("private"));
}
#[tokio::test]
async fn retry_after_status_etags_and_invalid_inputs_are_explicit() {
  assert!(Executor::builder().max_requests(0).build().is_err());
  let server = Server::start(|_| {
    let mut r = Reply::body(b"").header("Retry-After", "999");
    r.status = 429;
    r
  })
  .await;
  let http = Executor::builder().build().unwrap();
  assert!(
    matches!(http.get(&server.url).send().await,Err(Error::Status{code:429,retry_after:Some(d)}) if d==Duration::from_secs(30))
  );
  assert!(
    http
      .get("https://user:password@example.com/file")
      .send()
      .await
      .is_err()
  );
  assert!(
    http
      .get_range(&server.url, std::ops::Range { start: 4, end: 3 }, 6)
      .send()
      .await
      .is_err()
  );
  let server = Server::start(|r| Reply::range(r, b"abc").header("ETag", "\"new\"")).await;
  assert!(
    http
      .get_range(&server.url, 0..2, 3)
      .if_range("\"old\"")
      .send()
      .await
      .is_err()
  );
}

#[tokio::test]
async fn collection_cannot_raise_body_limit_and_invalid_etags_are_not_reused() {
  let server = Server::start(|_| {
    let mut reply = Reply::body(b"abcde").header("ETag", "\"");
    reply.length = None;
    reply
  })
  .await;
  let http = Executor::builder().build().unwrap();
  let response = http.get(&server.url).max_body_size(3).send().await.unwrap();
  assert!(response.etag().is_none());
  assert!(matches!(
    response.collect_exact(5).await,
    Err(Error::Invalid(_))
  ));
  assert!(
    http
      .get_range(&server.url, 0..2, 5)
      .if_range("W/\"weak\"")
      .send()
      .await
      .is_err()
  );
}

#[tokio::test]
async fn collection_after_stream_consumption_does_not_report_partial_bytes_as_exact() {
  let server = Server::start(|_| Reply::body(b"abcde")).await;
  let http = Executor::builder().build().unwrap();
  let mut response = http.get(&server.url).expected_size(5).send().await.unwrap();
  assert!(!response.chunk().await.unwrap().unwrap().is_empty());
  assert!(matches!(
    response.collect_exact(5).await,
    Err(Error::Invalid(_))
  ));
  let mut response = http.get(&server.url).send().await.unwrap();
  response.chunk().await.unwrap();
  assert!(matches!(
    response.collect_bounded(5).await,
    Err(Error::Invalid(_))
  ));
  assert_eq!(http.active_requests(), 0);
}

#[tokio::test]
async fn url_policy_rejects_initial_and_each_redirect_before_network() {
  let server = Server::start(|request| {
    let mut reply = Reply::body(b"");
    reply.status = 302;
    reply.headers.push((
      "Location".into(),
      if request.path == "/start" {
        "/middle"
      } else {
        "/blocked?grant=secret"
      }
      .into(),
    ));
    reply
  })
  .await;
  let http = Executor::builder()
    .url_policy(|url| url.path() != "/blocked")
    .build()
    .unwrap();
  let error = http
    .get(format!("{}/blocked?grant=secret", server.url))
    .send()
    .await
    .err()
    .unwrap();
  assert!(matches!(
    error,
    Error::Invalid("request URL rejected by policy")
  ));
  assert!(!error.to_string().contains("secret"));
  assert!(server.requests.lock().unwrap().is_empty());
  assert!(
    http
      .get(format!("{}/start", server.url))
      .send()
      .await
      .is_err()
  );
  let requests = server.requests.lock().unwrap();
  assert_eq!(
    requests
      .iter()
      .map(|request| request.path.as_str())
      .collect::<Vec<_>>(),
    ["/start", "/middle"]
  );
  assert_eq!(http.active_requests(), 0);
}

#[tokio::test]
async fn derived_url_policy_intersects_parent_and_shares_admission() {
  let server = Server::start(|_| Reply::body(b"body")).await;
  let parent = Executor::builder()
    .max_requests(1)
    .url_policy(|url| url.path() != "/parent-denied")
    .build()
    .unwrap();
  let child = parent.with_url_policy(|url| url.path() != "/child-denied");
  for path in ["parent-denied", "child-denied"] {
    assert!(
      child
        .get(format!("{}/{path}", server.url))
        .send()
        .await
        .is_err()
    );
  }
  assert!(server.requests.lock().unwrap().is_empty());
  let response = child.get(&server.url).send().await.unwrap();
  assert_eq!(parent.active_requests(), 1);
  assert!(
    tokio::time::timeout(Duration::from_millis(20), parent.get(&server.url).send())
      .await
      .is_err()
  );
  drop(response);
  assert_eq!(parent.active_requests(), 0);
  assert!(
    parent
      .get(format!("{}/child-denied", server.url))
      .send()
      .await
      .is_ok()
  );
}

#[tokio::test]
async fn allowed_cross_origin_redirect_and_ranges_still_work() {
  let target = Server::start(|r| Reply::range(r, b"abcdef")).await;
  let destination = target.url.clone();
  let source = Server::start(move |_| {
    let mut reply = Reply::body(b"");
    reply.status = 302;
    reply.headers.push(("Location".into(), destination.clone()));
    reply
  })
  .await;
  let source_port = url::Url::parse(&source.url).unwrap().port();
  let target_port = url::Url::parse(&target.url).unwrap().port();
  let http = Executor::builder()
    .url_policy(move |url| url.port() == source_port || url.port() == target_port)
    .build()
    .unwrap();
  let body = http
    .get_range(&source.url, 1..4, 6)
    .send()
    .await
    .unwrap()
    .collect_exact(3)
    .await
    .unwrap();
  assert_eq!(&body[..], b"bcd");
  assert_eq!(
    target.requests.lock().unwrap()[0].range.as_deref(),
    Some("bytes=1-3")
  );
  assert_eq!(http.active_requests(), 0);
}
