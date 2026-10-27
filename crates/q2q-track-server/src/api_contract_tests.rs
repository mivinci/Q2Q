//! The native HTTP boundary consumes the same fixture as the Workers implementation.
use super::*;
use axum::{
  body::{Body, to_bytes},
  http::Request,
};
use q2q_ratelimit::PeerAddr;
use q2q_track::state::MemoryState;
use tower::ServiceExt;

#[tokio::test]
async fn shared_http_contract() {
  let fixture: serde_json::Value =
    serde_json::from_str(include_str!("../../../tests/fixtures/track/v1.json")).unwrap();
  let app = router_with(
    Arc::new(MemoryState::new()),
    Arc::new(TrackConfig::default()),
    PunchList::new(vec![]),
    limiter(),
  );
  for case in fixture["http"].as_array().unwrap() {
    let request = Request::builder()
      .method(case["method"].as_str().unwrap())
      .uri(case["uri"].as_str().unwrap())
      .header("content-type", "application/json")
      .extension(PeerAddr("127.0.0.1".parse().unwrap()))
      .body(Body::from(case["body"].as_str().unwrap().to_owned()))
      .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(
      response.status().as_u16(),
      case["status"].as_u64().unwrap() as u16,
      "{case}"
    );
    if let Some(code) = case["error"].as_str() {
      let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
      assert_eq!(body["code"], code, "{case}");
    }
  }
  // Authentication cases have fixed fixture time; exercise the HTTP error mapping
  // directly instead of depending on the machine clock in the announce handler.
  for case in fixture["tickets"].as_array().unwrap() {
    let body = serde_json::from_str(case["body_json"].as_str().unwrap()).unwrap();
    let now = case["now"].as_str().unwrap().parse().unwrap();
    if let Err(error) = auth::verify_announce(&body, now) {
      let (status, Json(body)) = auth_failure(error);
      assert_eq!(
        status.as_u16(),
        case["http_status"].as_u64().unwrap() as u16,
        "{case}"
      );
      assert_eq!(body.code, case["error"].as_str().unwrap(), "{case}");
    }
  }
}
