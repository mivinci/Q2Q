//! The public Track types consume the same Punch contract as its dedicated crate.
use q2q_track::{
  state::PunchAssignment,
  wire::{AnnounceRequest, PunchWire},
};
use serde_json::{Value, json};

#[test]
fn track_wire_and_stored_assignment_share_canonical_endpoint_vectors() {
  let vectors: Value = serde_json::from_str(include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/punch/endpoints.json"
  )))
  .unwrap();
  for vector in vectors["sets"].as_array().unwrap() {
    let wire = json!({"punch_id":"test","endpoints":vector["input"],"public_key":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="});
    let state = json!({"punch_id":"test","endpoints":vector["input"],"public_key":vec![0;32]});
    let wire = serde_json::from_value::<PunchWire>(wire);
    let state = serde_json::from_value::<PunchAssignment>(state);
    if vector["canonical"].is_null() {
      assert!(wire.is_err());
      assert!(state.is_err());
    } else {
      assert_eq!(
        serde_json::to_value(wire.unwrap()).unwrap()["endpoints"],
        vector["canonical"]
      );
      assert_eq!(
        serde_json::to_value(state.unwrap()).unwrap()["endpoints"],
        vector["canonical"]
      );
    }
  }
}
#[test]
fn announce_requires_scheme_capabilities_and_rejects_old_transport_field() {
  let fixture: Value = serde_json::from_str(include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/track/v1.json"
  )))
  .unwrap();
  let token: Value =
    serde_json::from_str(fixture["tickets"][0]["body_json"].as_str().unwrap()).unwrap();
  let base = json!({"token":token,"ttl":300,"punch_schemes":["wss","quic"]});
  let value: AnnounceRequest = serde_json::from_value(base.clone()).unwrap();
  assert_eq!(
    serde_json::to_value(value).unwrap()["punch_schemes"],
    json!(["quic", "wss"])
  );
  let mut missing = base.clone();
  missing.as_object_mut().unwrap().remove("punch_schemes");
  assert!(serde_json::from_value::<AnnounceRequest>(missing).is_err());
  let mut old = base.clone();
  old["punch_transport"] = json!("websocket");
  assert!(serde_json::from_value::<AnnounceRequest>(old).is_err());
  let mut duplicate = base;
  duplicate["punch_schemes"] = json!(["wss", "wss"]);
  assert!(serde_json::from_value::<AnnounceRequest>(duplicate).is_err());
}
