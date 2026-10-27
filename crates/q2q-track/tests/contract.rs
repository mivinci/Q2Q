//! Cross-runtime fixtures. Regeneration is explicit; normal tests never rewrite expectations.
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use q2q_primitives::{
  bencode::{BValue, Dict},
  identity::{Ticket, TicketBody, digest_payload, keypair_from_seed, peer_id_from_public_key},
};
use q2q_track::{
  auth,
  state::{MemoryState, OnlineRecord, PunchAssignment, StateError, TrackState},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const FIXTURE: &str = concat!(
  env!("CARGO_MANIFEST_DIR"),
  "/../../tests/fixtures/track/v1.json"
);

fn canonical(body: &TicketBody) -> Vec<u8> {
  let ticket = body.to_ticket().unwrap();
  let mut entries = Dict::new();
  for (name, value) in [
    ("v", body.v as i64),
    ("iat", body.iat as i64),
    ("exp", body.exp as i64),
  ] {
    entries.insert(name.as_bytes().to_vec(), BValue::Int(value));
  }
  for (name, bytes) in [
    ("iss", body.iss.as_bytes()),
    ("pub", ticket.public_key.as_slice()),
    ("aud", body.aud.as_bytes()),
  ] {
    entries.insert(name.as_bytes().to_vec(), BValue::Bytes(bytes.to_vec()));
  }
  for (name, value) in [("tgt", body.tgt.as_deref()), ("cid", body.cid.as_deref())] {
    if let Some(value) = value {
      entries.insert(
        name.as_bytes().to_vec(),
        BValue::Bytes(value.as_bytes().to_vec()),
      );
    }
  }
  if let Some(hash) = ticket.ih {
    entries.insert(b"ih".to_vec(), BValue::Bytes(hash.to_vec()));
  }
  let bytes = BValue::Dict(entries).encode();
  assert_eq!(Sha256::digest(&bytes).as_slice(), digest_payload(&ticket));
  bytes
}

fn ticket(iat: u64, exp: u64, aud: &str) -> Ticket {
  let (sk, peer) = keypair_from_seed(&[7; 32]);
  let mut ticket = Ticket {
    v: 2,
    iss: peer,
    public_key: sk.verifying_key().to_bytes(),
    iat,
    exp,
    aud: aud.into(),
    tgt: None,
    ih: None,
    cid: None,
    sig: [0; 64],
  };
  ticket.sign(&sk);
  ticket
}

fn ticket_case(id: &str, body: TicketBody, now: u64) -> Value {
  let payload =
    if body.iat <= i64::MAX as u64 && body.exp <= i64::MAX as u64 && body.to_ticket().is_ok() {
      Some(B64.encode(canonical(&body)))
    } else {
      None
    };
  let expected = auth::verify_announce(&body, now)
    .err()
    .map(|error| error.code());
  let http_status = match expected {
    None => 200,
    Some("malformed_ticket") => 400,
    Some(
      "bad_signature" | "key_not_issuer" | "expired" | "lifetime_too_long" | "wrong_audience",
    ) => 401,
    _ => 500,
  };
  json!({ "id": id, "body_json": serde_json::to_string(&body).unwrap(), "now": now.to_string(),
    "canonical_base64": payload, "error": expected, "http_status": http_status })
}

fn crypto_vectors() -> Value {
  let now = 1_000_000;
  let valid = ticket(now, now + 60, "track");
  let mut cases = vec![
    ticket_case("valid", valid.to_body(), now),
    ticket_case("expiry-exclusive", valid.to_body(), now + 60),
    ticket_case(
      "remaining-lifetime-600",
      ticket(now, now + 600, "track").to_body(),
      now,
    ),
    ticket_case(
      "remaining-lifetime-601",
      ticket(now, now + 601, "track").to_body(),
      now,
    ),
    ticket_case(
      "future-iat-is-currently-accepted",
      ticket(now + 10, now + 60, "track").to_body(),
      now,
    ),
    ticket_case(
      "old-iat-with-short-remaining-life",
      ticket(1, now + 60, "track").to_body(),
      now,
    ),
    ticket_case(
      "wrong-audience",
      ticket(now, now + 60, "punch:test").to_body(),
      now,
    ),
    ticket_case(
      "time-i64-max",
      ticket(i64::MAX as u64 - 60, i64::MAX as u64, "track").to_body(),
      i64::MAX as u64 - 60,
    ),
  ];
  let mut optional = ticket(now, now + 60, "track");
  optional.ih = Some([0xa5; 32]);
  optional.cid = Some("连接-café-🌱".into());
  optional.sign(&keypair_from_seed(&[7; 32]).0);
  cases.push(ticket_case(
    "optional-binary-and-utf8",
    optional.to_body(),
    now,
  ));
  optional.tgt = Some(valid.iss.clone());
  optional.sign(&keypair_from_seed(&[7; 32]).0);
  cases.push(ticket_case(
    "track-ticket-cannot-name-target",
    optional.to_body(),
    now,
  ));
  for (id, body) in [
    ("bad-signature", {
      let mut b = valid.to_body();
      let mut sig = valid.sig;
      sig[0] ^= 1;
      b.sig = B64.encode(sig);
      b
    }),
    ("noncanonical-signature-scalar", {
      let mut b = valid.to_body();
      let mut sig = valid.sig;
      sig[63] = 255;
      b.sig = B64.encode(sig);
      b
    }),
    ("invalid-compressed-public-key", {
      let mut b = valid.to_body();
      b.public_key = B64.encode([2; 32]);
      b.iss = peer_id_from_public_key(&[2; 32]).to_string();
      b
    }),
    ("wrong-issuer-key", {
      let mut b = valid.to_body();
      b.public_key = B64.encode([9; 32]);
      b
    }),
    ("unsupported-version", {
      let mut b = valid.to_body();
      b.v = 1;
      b
    }),
    ("inverted-validity", {
      let mut b = valid.to_body();
      b.iat = b.exp + 1;
      b
    }),
    ("time-u64-max-rejected-without-panic", {
      let mut b = valid.to_body();
      b.exp = u64::MAX;
      b
    }),
    ("public-key-too-short", {
      let mut b = valid.to_body();
      b.public_key = B64.encode([9; 31]);
      b
    }),
    ("signature-too-short", {
      let mut b = valid.to_body();
      b.sig = B64.encode([9; 63]);
      b
    }),
    ("base64-padding-required", {
      let mut b = valid.to_body();
      b.public_key = b.public_key.trim_end_matches('=').into();
      b
    }),
  ] {
    cases.push(ticket_case(id, body, now));
  }

  let peer = valid.iss;
  let key = [17; 32];
  let mut sessions = Vec::new();
  for (id, seq, timestamp, last_seq, corrupt) in [
    ("valid", 1, now, 0, false),
    ("clock-minus-60", 1, now - 60, 0, false),
    ("clock-plus-60", 1, now + 60, 0, false),
    ("clock-minus-61", 1, now - 61, 0, false),
    ("clock-plus-61", 1, now + 61, 0, false),
    ("duplicate-needs-storage-check", 10, now, 10, false),
    ("seven-behind-needs-storage-check", 3, now, 10, false),
    ("eight-behind", 2, now, 10, false),
    (
      "above-js-safe-integer",
      9_007_199_254_740_993,
      now,
      0,
      false,
    ),
    ("above-sqlite-signed-integer", 1u64 << 63, now, 0, false),
    ("u64-max-no-overflow", u64::MAX, now, 0, false),
    (
      "u64-max-duplicate-needs-storage-check",
      u64::MAX,
      now,
      u64::MAX,
      false,
    ),
    ("bad-mac", 1, now, 0, true),
  ] {
    let mut mac = auth::mac_bytes(&key, &peer, seq, timestamp);
    if corrupt {
      mac[0] ^= 1;
    }
    let mac = B64.encode(mac);
    let error = auth::verify_session_request(&key, &peer, seq, timestamp, &mac, last_seq, now)
      .err()
      .map(|error| error.code());
    let payload = [
      peer.as_str().as_bytes(),
      &seq.to_be_bytes(),
      &timestamp.to_be_bytes(),
    ]
    .concat();
    sessions.push(
      json!({ "id": id, "peer_id": peer.to_string(), "key_base64": B64.encode(key),
      "seq": seq.to_string(), "timestamp": timestamp.to_string(), "last_seq": last_seq.to_string(),
      "now": now.to_string(), "mac": mac, "payload_base64": B64.encode(payload), "error": error }),
    );
  }
  let identities: Vec<Value> = [0, 7, 255].into_iter().map(|seed| {
    let (key, peer) = keypair_from_seed(&[seed; 32]);
    json!({ "public_key": B64.encode(key.verifying_key().to_bytes()), "peer_id": peer.to_string() })
  }).collect();
  let username = format!("1003600:{peer}");
  let turn = json!({ "secret": "fixture-only-turn-secret", "username": username,
    "credential": B64.encode(q2q_primitives::mac::sha1(b"fixture-only-turn-secret", &[username.as_bytes()])) });
  let integers: Vec<Value> = [
    "0",
    "1",
    "9007199254740991",
    "9007199254740993",
    "9223372036854775807",
    "9223372036854775808",
    "18446744073709551615",
    "18446744073709551616",
    "-1",
    "-0",
    "1.0",
    "1e0",
    "\"1\"",
    "null",
    "true",
  ]
  .into_iter()
  .map(|raw| {
    let body = format!("{{\"seq\":{raw},\"timestamp\":1,\"mac\":\"\"}}");
    let expected = serde_json::from_str::<q2q_track::wire::HeartbeatRequest>(&body)
      .ok()
      .map(|req| req.seq.to_string());
    json!({ "body_json": body, "value": expected })
  })
  .collect();
  let http = json!([
    {"method":"GET","uri":"/health","body":"","status":200,"error":null},
    {"method":"GET","uri":"/files/not-a-hash/peers","body":"","status":400,"error":"bad_request"},
    {"method":"GET","uri":format!("/files/sha256:{}/peers", "01".repeat(32)),"body":"","status":401,"error":"unknown_session"},
    {"method":"GET","uri":format!("/files/sha256:{}/peers?limit=abc", "01".repeat(32)),"body":"","status":400,"error":null},
    {"method":"GET","uri":format!("/files/sha256:{}/peers?limit=1&limit=2", "01".repeat(32)),"body":"","status":400,"error":null},
    {"method":"GET","uri":format!("/files/sha256:{}/peers?limit=%2B1", "01".repeat(32)),"body":"","status":401,"error":"unknown_session"},
    {"method":"GET","uri":format!("/files/sha256:{}/peers?seq=0000000000000000000000000001", "01".repeat(32)),"body":"","status":401,"error":"unknown_session"},
    {"method":"GET","uri":format!("/files/sha256:{}/peers?seq=18446744073709551616", "01".repeat(32)),"body":"","status":400,"error":null},
    {"method":"POST","uri":"/peers/test/heartbeat","body":"{}","status":422,"error":null},
    {"method":"POST","uri":"/peers/test/heartbeat","body":"{\"seq\":-1,\"timestamp\":1,\"mac\":\"\"}","status":422,"error":null},
    {"method":"POST","uri":"/peers/test/heartbeat","body":"{\"seq\":1,\"seq\":2,\"timestamp\":1,\"mac\":\"\"}","status":422,"error":null},
    {"method":"PUT","uri":"/peers/test/files","body":"{}","status":422,"error":null},
    {"method":"POST","uri":"/health","body":"","status":405,"error":null}
  ]);
  json!({ "schema_version": 1, "identities": identities, "tickets": cases, "sessions": sessions, "integers": integers, "turn": turn, "http": http })
}

fn n(step: &Value, key: &str) -> u64 {
  step[key].as_str().unwrap().parse().unwrap()
}
fn hashes(step: &Value, field: &str) -> Vec<[u8; 32]> {
  step[field]
    .as_array()
    .unwrap()
    .iter()
    .map(|v| [v.as_u64().unwrap() as u8; 32])
    .collect()
}

async fn state_vectors() -> Value {
  let (_, peer) = keypair_from_seed(&[7; 32]);
  let sequences = vec![
    (
      "delete-before-add-and-independent-ttls",
      vec![
        json!({"op":"announce","key":7,"expiry":"150","add":[1,2],"del":[1],"complete":[1]}),
        json!({"op":"query","hash":1,"now":"100"}),
        json!({"op":"advance","key":7,"seq":"1","now":"100","renew":"300"}),
        json!({"op":"query","hash":1,"now":"150"}),
        json!({"op":"online","now":"150"}),
      ],
    ),
    (
      "reannounce-retains-holdings-and-invalidates-session",
      vec![
        json!({"op":"announce","key":7,"expiry":"300","add":[1,2],"del":[],"complete":[1]}),
        json!({"op":"advance","key":7,"seq":"10","now":"100","renew":null}),
        json!({"op":"announce","key":8,"expiry":"400","add":[],"del":[2],"complete":[]}),
        json!({"op":"advance","key":7,"seq":"11","now":"100","renew":null}),
        json!({"op":"advance","key":8,"seq":"1","now":"100","renew":null}),
        json!({"op":"query","hash":1,"now":"100"}),
        json!({"op":"query","hash":2,"now":"100"}),
      ],
    ),
    (
      "strict-sequence-and-expiry-boundary",
      vec![
        json!({"op":"announce","key":7,"expiry":"150","add":[1],"del":[],"complete":[]}),
        json!({"op":"advance","key":7,"seq":"1","now":"100","renew":null}),
        json!({"op":"advance","key":7,"seq":"1","now":"100","renew":null}),
        json!({"op":"advance","key":7,"seq":"0","now":"100","renew":null}),
        json!({"op":"advance","key":7,"seq":"18446744073709551615","now":"100","renew":null}),
        json!({"op":"advance","key":7,"seq":"18446744073709551615","now":"100","renew":null}),
        json!({"op":"advance","key":7,"seq":"18446744073709551615","now":"150","renew":"300"}),
      ],
    ),
  ];
  let mut traces = Vec::new();
  for (name, steps) in sequences {
    let state = MemoryState::new();
    let mut outputs = Vec::new();
    for step in &steps {
      let output = match step["op"].as_str().unwrap() {
        "announce" => {
          state
            .announce(
              OnlineRecord {
                peer_id: peer.clone(),
                public_key: [7; 32],
                session_key: [step["key"].as_u64().unwrap() as u8; 32],
                last_seq: 0,
                punch: PunchAssignment {
                  punch_id: "fixture-punch".into(),
                  url: "quic://punch.invalid:443".into(),
                  public_key: vec![9; 32],
                },
                expires_at: n(step, "expiry"),
              },
              &hashes(step, "add"),
              &hashes(step, "del"),
              &hashes(step, "complete"),
              300,
            )
            .await
            .unwrap();
          json!("ok")
        }
        "advance" => {
          match state
            .advance_session(
              &peer,
              &[step["key"].as_u64().unwrap() as u8; 32],
              n(step, "seq"),
              step["renew"].as_str().map(|v| v.parse().unwrap()),
              n(step, "now"),
            )
            .await
          {
            Ok(()) => json!("ok"),
            Err(StateError::UnknownSession) => json!("unknown_session"),
            Err(StateError::Replayed(_)) => json!("bad_mac"),
            Err(e) => panic!("{e:?}"),
          }
        }
        "query" => json!(
          state
            .query(
              &[step["hash"].as_u64().unwrap() as u8; 32],
              10,
              None,
              n(step, "now")
            )
            .await
            .unwrap()
            .into_iter()
            .map(|c| json!({ "peer_id": c.peer_id.to_string(), "complete":c.complete }))
            .collect::<Vec<_>>()
        ),
        "online" => json!(
          state
            .online(&peer, n(step, "now"))
            .await
            .unwrap()
            .map(|r| json!({"seq":r.last_seq.to_string(),"expiry":r.expires_at.to_string()}))
        ),
        _ => unreachable!(),
      };
      outputs.push(output);
    }
    traces.push(json!({"id":name,"steps":steps,"outputs":outputs}));
  }
  json!(traces)
}

async fn selection_vectors() -> Value {
  let state = MemoryState::new();
  let mut members = Vec::new();
  for n in 0u8..16 {
    let peer = peer_id_from_public_key(&[n; 32]);
    let punch_id = if n % 5 == 0 { "retired" } else { "live" };
    let expiry = if n == 1 { 50 } else { 1000 };
    state
      .announce(
        OnlineRecord {
          peer_id: peer.clone(),
          public_key: [n; 32],
          session_key: [7; 32],
          last_seq: 0,
          punch: PunchAssignment {
            punch_id: punch_id.into(),
            url: "quic://punch.invalid:443".into(),
            public_key: vec![9; 32],
          },
          expires_at: expiry,
        },
        &[[1; 32]],
        &[],
        if n % 2 == 0 { &[[1; 32]] } else { &[] },
        1000,
      )
      .await
      .unwrap();
    members.push(json!({"peer_id": peer.to_string(), "public_key": B64.encode([n; 32]), "punch_id": punch_id, "expires_at": expiry, "complete": n % 2 == 0}));
  }
  let requester = peer_id_from_public_key(&[2; 32]);
  let start = peer_id_from_public_key(&[8; 32]);
  let punches = std::collections::HashSet::from(["live".to_string()]);
  let mut cases = Vec::new();
  for legacy in [false, true] {
    let mut cursor = legacy.then(|| start.to_string());
    let initial = cursor.clone();
    let mut pages = Vec::new();
    loop {
      let result = q2q_track::selection::Query {
        info_hash: &[1; 32],
        requester: &requester,
        limit: 3,
        cursor: cursor.as_deref(),
        punch_ids: &punches,
        now: 100,
      }
      .select(&state, start.clone())
      .await
      .unwrap();
      cursor = result.next_cursor;
      pages.push(json!({"peers": result.peers.iter().map(|p| json!({"peer_id": p.peer_id.to_string(), "complete": p.complete})).collect::<Vec<_>>(), "next_cursor": cursor}));
      if cursor.is_none() {
        break;
      }
      assert!(pages.len() < 16);
    }
    cases.push(json!({"initial_cursor": initial, "pages": pages}));
  }
  json!({"members": members, "requester": requester.to_string(), "start": start.to_string(), "limit": 3, "now": 100, "cases": cases})
}

async fn vectors() -> Value {
  let mut fixture = crypto_vectors();
  fixture["state_traces"] = state_vectors().await;
  fixture["selection"] = selection_vectors().await;
  fixture
}

#[tokio::test]
async fn native_contract_matches_shared_fixture() {
  let committed: Value = serde_json::from_str(&std::fs::read_to_string(FIXTURE).unwrap()).unwrap();
  assert_eq!(
    vectors().await,
    committed,
    "native behavior changed; inspect compatibility before regenerating"
  );
}

#[tokio::test]
#[ignore = "explicit fixture generation, never part of normal validation"]
async fn regenerate_protocol_fixture() {
  std::fs::create_dir_all(std::path::Path::new(FIXTURE).parent().unwrap()).unwrap();
  std::fs::write(
    FIXTURE,
    format!(
      "{}\n",
      serde_json::to_string_pretty(&vectors().await).unwrap()
    ),
  )
  .unwrap();
}
