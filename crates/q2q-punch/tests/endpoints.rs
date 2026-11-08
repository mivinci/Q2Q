use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use q2q_primitives::identity::{keypair_from_seed, public_key_of, sign_message, verify_signature};
use q2q_punch::endpoints::{
  PunchAssignment, PunchEndpoints, PunchIdentity, PunchSchemes, canonical_endpoint,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

const VECTORS: &str = include_str!(concat!(
  env!("CARGO_MANIFEST_DIR"),
  "/../../tests/fixtures/punch/endpoints.json"
));
const CREDENTIAL: &str = include_str!(concat!(
  env!("CARGO_MANIFEST_DIR"),
  "/../../tests/fixtures/punch/credential.json"
));

fn credential() -> Value {
  serde_json::from_str(CREDENTIAL).unwrap()
}

#[test]
fn shared_endpoint_vectors() {
  let vectors: Value = serde_json::from_str(VECTORS).unwrap();
  for vector in vectors["endpoints"].as_array().unwrap() {
    let name = vector["name"].as_str().unwrap();
    let actual = canonical_endpoint(vector["input"].as_str().unwrap());
    if let Some(expected) = vector["canonical"].as_str() {
      assert_eq!(actual.as_ref().unwrap(), expected, "{name}");
      assert_eq!(canonical_endpoint(expected).unwrap(), expected, "{name}");
    } else {
      assert!(actual.is_err(), "{name}");
    }
  }
}

fn check_sets<T: DeserializeOwned + serde::Serialize>(vectors: &Value, group: &str) {
  for vector in vectors[group].as_array().unwrap() {
    let name = vector["name"].as_str().unwrap();
    let actual = serde_json::from_value::<T>(vector["input"].clone());
    if vector["canonical"].is_null() {
      assert!(actual.is_err(), "{name}");
    } else {
      assert_eq!(
        serde_json::to_value(actual.unwrap()).unwrap(),
        vector["canonical"],
        "{name}"
      );
    }
  }
}

#[test]
fn shared_endpoint_set_and_capability_vectors() {
  let vectors: Value = serde_json::from_str(VECTORS).unwrap();
  check_sets::<PunchEndpoints>(&vectors, "sets");
  check_sets::<PunchSchemes>(&vectors, "schemes");
}

#[test]
fn count_length_and_type_bounds() {
  let prefix = "wss://punch.invalid/";
  assert!(canonical_endpoint(&format!("{prefix}{}", "x".repeat(2048 - prefix.len()))).is_ok());
  assert!(canonical_endpoint(&format!("{prefix}{}", "x".repeat(2049 - prefix.len()))).is_err());
  assert!(canonical_endpoint(&format!("{prefix}{}", "中".repeat(300))).is_err());
  for value in [Value::Null, json!(1), json!(true), json!({}), json!([])] {
    assert!(serde_json::from_value::<PunchEndpoints>(json!([value])).is_err());
  }
  let mut values: Vec<_> = (0..8).map(|n| format!("{prefix}{n}")).collect();
  assert_eq!(
    PunchEndpoints::try_from(values.clone())
      .unwrap()
      .as_slice()
      .len(),
    8
  );
  values.push(format!("{prefix}8"));
  assert!(PunchEndpoints::try_from(values).is_err());
}

fn identity(claims: &Value) -> Value {
  json!({"endpoints":claims["endpoints"], "public_key":claims["public_key"], "identity_version":claims["identity_version"]})
}
fn assignment(claims: &Value) -> Value {
  json!({"punch_id":claims["sub"], "endpoints":claims["endpoints"], "public_key":claims["public_key"]})
}

#[test]
fn list_only_identity_and_assignment_reject_legacy_fields() {
  let vector = credential();
  let claims = &vector["claims"];
  let identity = identity(claims);
  let assignment = assignment(claims);
  let expected = serde_json::from_value::<PunchIdentity>(identity.clone()).unwrap();
  let node = serde_json::from_value::<PunchAssignment>(assignment.clone()).unwrap();
  assert_eq!(expected.endpoints, node.endpoints);
  let mut reordered = identity.clone();
  reordered["endpoints"].as_array_mut().unwrap().reverse();
  assert_eq!(
    serde_json::from_value::<PunchIdentity>(reordered).unwrap(),
    expected
  );
  for field in ["url", "advertise_url", "websocket_url"] {
    let mut value = identity.clone();
    value[field] = "quic://old.invalid:4433".into();
    assert!(serde_json::from_value::<PunchIdentity>(value).is_err());
    let mut value = assignment.clone();
    value[field] = "quic://old.invalid:4433".into();
    assert!(serde_json::from_value::<PunchAssignment>(value).is_err());
  }
  let mut value = assignment.clone();
  value.as_object_mut().unwrap().remove("endpoints");
  value["url"] = "quic://old.invalid:4433".into();
  assert!(serde_json::from_value::<PunchAssignment>(value).is_err());
}

#[test]
fn identity_field_validation_matches_typescript() {
  let vector = credential();
  let identity = identity(&vector["claims"]);
  let assignment = assignment(&vector["claims"]);
  for id in [String::new(), "../node".into(), "x".repeat(129)] {
    let mut value = assignment.clone();
    value["punch_id"] = id.into();
    assert!(serde_json::from_value::<PunchAssignment>(value).is_err());
  }
  for key in [
    String::new(),
    "invalid".into(),
    "A".repeat(43),
    format!("{}B=", "A".repeat(42)),
  ] {
    let mut value = assignment.clone();
    value["public_key"] = key.into();
    assert!(serde_json::from_value::<PunchAssignment>(value).is_err());
  }
  for version in [
    json!(0),
    json!(-1),
    json!(1.5),
    json!(9_007_199_254_740_992_u64),
    json!("1"),
  ] {
    let mut value = identity.clone();
    value["identity_version"] = version;
    assert!(serde_json::from_value::<PunchIdentity>(value).is_err());
  }
  let mut value = identity;
  value["identity_version"] = json!(9_007_199_254_740_991_u64);
  assert!(serde_json::from_value::<PunchIdentity>(value).is_ok());
}

#[test]
fn shared_jwt_signature_binds_the_exact_raw_endpoint_identity() {
  let vector = credential();
  assert_eq!(vector["seed_hex"], "2a".repeat(32));
  let (key, _) = keypair_from_seed(&[42; 32]);
  let public = public_key_of(&key);
  assert_eq!(B64.encode(public), vector["public_jwk"]["x"]);
  let token = vector["token"].as_str().unwrap();
  let (input, signature) = token.rsplit_once('.').unwrap();
  let signature: [u8; 64] = B64.decode(signature).unwrap().try_into().unwrap();
  assert!(verify_signature(&public, input.as_bytes(), &signature));
  assert_eq!(sign_message(&key, input.as_bytes()), signature);
  let (header, body) = input.split_once('.').unwrap();
  assert_eq!(
    serde_json::from_slice::<Value>(&B64.decode(header).unwrap()).unwrap(),
    vector["header"]
  );
  let claims: Value = serde_json::from_slice(&B64.decode(body).unwrap()).unwrap();
  assert_eq!(claims, vector["claims"]);
  for change in [
    json!({"endpoints":[claims["endpoints"][1],claims["endpoints"][0]]}),
    json!({"endpoints":[claims["endpoints"][0]]}),
    json!({"endpoints":[claims["endpoints"][0],"wss://proxy.invalid:8443/changed"]}),
    json!({"public_key":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="}),
    json!({"identity_version":3}),
  ] {
    let mut changed = claims.clone();
    changed
      .as_object_mut()
      .unwrap()
      .extend(change.as_object().unwrap().clone());
    let changed = format!(
      "{header}.{}",
      B64.encode(serde_json::to_vec(&changed).unwrap())
    );
    assert!(!verify_signature(&public, changed.as_bytes(), &signature));
  }
}

#[test]
fn endpoint_errors_do_not_echo_query_credentials() {
  let error = canonical_endpoint("wss://user@punch.invalid/ws?secret=do-not-echo").unwrap_err();
  assert!(!error.to_string().contains("do-not-echo"));
}
