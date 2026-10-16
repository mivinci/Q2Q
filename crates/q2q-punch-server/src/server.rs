//! QUIC transport for the binding RPCs.
//!
//! Two paths, because the traffic is not the same shape:
//!
//! - `join` / `exit` are request-response and must arrive → bidirectional stream
//! - `heartbeat` is a small packet every few seconds, and a lost one is simply
//!   followed by another → datagram
//!
//! QUIC is the right substrate for both: it is UDP, which is what keeps a NAT
//! mapping alive, and a connection survives the client changing network
//! (WiFi ↔ cellular) because it is identified by a connection id rather than by
//! the four-tuple. That second property is why `transport_generation` exists at
//! all.

use crate::cert::{CertError, PunchIdentity};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use q2q_punch::binding::{Binding, Bindings, UnixSeconds, now};
use q2q_punch::message::{
  CLOCK_TOLERANCE, ExitRequest, HeartbeatRequest, HeartbeatResponse, JoinRequest, JoinResponse,
  Method, Request, Response, parse_request, verify_binding_mac,
};
use quinn::{Connection, Endpoint};
use std::net::SocketAddr;
use std::sync::Arc;

pub struct State {
  pub bindings: Bindings,
  /// Tickets must name this exact server, so one leak cannot be replayed
  /// against a different Punch node.
  pub punch_id: String,
  pub ttl: UnixSeconds,
  pub max_request_bytes: usize,
}

// ------------------------------------------------------------------- dispatch

/// Handle one request. Pure apart from the binding table and the rng, so it can
/// be tested without a socket.
pub fn dispatch(state: &State, req: &Request, now: UnixSeconds) -> Response {
  let Some(method) = Method::parse(&req.method) else {
    return Response::err(&req.id, "unknown_method", Some(req.method.clone()));
  };

  match method {
    Method::Join => match serde_json::from_value::<JoinRequest>(req.params.clone()) {
      Ok(join) => handle_join(state, req, &join, now),
      Err(e) => Response::err(&req.id, "malformed_params", Some(e.to_string())),
    },
    Method::Heartbeat => match serde_json::from_value::<HeartbeatRequest>(req.params.clone()) {
      Ok(hb) => handle_heartbeat(state, req, &hb, now),
      Err(e) => Response::err(&req.id, "malformed_params", Some(e.to_string())),
    },
    Method::Exit => match serde_json::from_value::<ExitRequest>(req.params.clone()) {
      Ok(ex) => handle_exit(state, req, &ex, now),
      Err(e) => Response::err(&req.id, "malformed_params", Some(e.to_string())),
    },
  }
}

fn handle_join(state: &State, req: &Request, join: &JoinRequest, now: UnixSeconds) -> Response {
  let ticket = match q2q_punch::message::verify_join(join, &state.punch_id, now) {
    Ok(t) => t,
    Err(e) => {
      let code = e.code();
      return Response::err(&req.id, code, Some(e.to_string()));
    }
  };

  // replay protection rests entirely on the one-shot ticket id: join has no
  // challenge, so a second join with the same cid must be refused
  let Some(cid) = ticket.cid.clone() else {
    return Response::err(
      &req.id,
      "malformed_ticket",
      Some("join ticket needs a cid".into()),
    );
  };
  if !state.bindings.spend(&cid, now, ticket.exp) {
    return Response::err(&req.id, "ticket_replayed", None);
  }

  let binding = Binding {
    binding_id: random_hex(16),
    peer_id: q2q_core::identity::PeerId::unchecked(&ticket.iss),
    key: random_key(),
    expires_at: now + state.ttl,
    last_seq: 0,
    generation: 0,
  };

  state.bindings.insert(binding.clone());
  tracing::info!(peer = %binding.peer_id.as_str(), punch = %state.punch_id, "binding established");

  Response::ok(
    &req.id,
    &JoinResponse {
      binding_id: binding.binding_id,
      binding_key: B64.encode(binding.key),
      expires_at: binding.expires_at,
    },
  )
}

fn handle_heartbeat(
  state: &State,
  req: &Request,
  hb: &HeartbeatRequest,
  now: UnixSeconds,
) -> Response {
  let binding = match authenticated(state, hb.binding_id.as_str(), now) {
    Ok(b) => b,
    Err(e) => return Response::err(&req.id, e, None),
  };
  if hb.timestamp.abs_diff(now) > CLOCK_TOLERANCE {
    return Response::err(&req.id, "stale_timestamp", None);
  }

  let mac_ok = verify_binding_mac(
    &binding.key,
    &hb.binding_id,
    hb.seq,
    hb.timestamp,
    hb.generation,
    &hb.mac,
  );
  match state
    .bindings
    .heartbeat(&binding.peer_id, hb.seq, hb.generation, mac_ok, now)
  {
    Ok(updated) => Response::ok(
      &req.id,
      &HeartbeatResponse {
        expires_at: updated.expires_at,
      },
    ),
    Err(e) => Response::err(&req.id, e.code(), Some(e.to_string())),
  }
}

fn handle_exit(state: &State, req: &Request, ex: &ExitRequest, now: UnixSeconds) -> Response {
  let binding = match authenticated(state, ex.binding_id.as_str(), now) {
    Ok(b) => b,
    Err(e) => return Response::err(&req.id, e, None),
  };
  if !verify_binding_mac(
    &binding.key,
    &ex.binding_id,
    ex.seq,
    ex.timestamp,
    ex.generation,
    &ex.mac,
  ) {
    return Response::err(&req.id, "bad_mac", None);
  }
  state.bindings.remove(&binding.peer_id);
  Response::ok(&req.id, &serde_json::json!({ "ok": true }))
}

/// Look up a live binding by id, reporting problems as error codes.
///
/// `now` is passed in rather than read from the clock, so tests can reason
/// about expiry without waiting for it.
fn authenticated(
  state: &State,
  binding_id: &str,
  now: UnixSeconds,
) -> Result<Binding, &'static str> {
  let binding = state.bindings.find(binding_id).ok_or("unknown_binding")?;
  if binding.expires_at <= now {
    return Err("binding_expired");
  }
  Ok(binding)
}

// ------------------------------------------------------------------ transport

pub fn endpoint(identity: &PunchIdentity, bind: SocketAddr) -> Result<Endpoint, CertError> {
  let config = identity.server_config()?;
  Endpoint::server(config, bind).map_err(|e| CertError::Generate(e.to_string()))
}

pub async fn serve(endpoint: Endpoint, state: Arc<State>) {
  while let Some(incoming) = endpoint.accept().await {
    let st = state.clone();
    match incoming.await {
      Ok(conn) => {
        tokio::spawn(handle_connection(conn, st));
      }
      Err(e) => tracing::warn!("incoming connection failed: {e}"),
    }
  }
}

async fn handle_connection(conn: Connection, state: Arc<State>) {
  let remote = conn.remote_address();
  tracing::debug!(%remote, "connection accepted");

  // heartbeats: loss is tolerated, so they do not need a stream
  let datagrams = conn.clone();
  let dg_state = state.clone();
  tokio::spawn(async move {
    while let Ok(bytes) = datagrams.read_datagram().await {
      let resp = match parse_request(&bytes) {
        Ok(req) => dispatch(&dg_state, &req, now()),
        Err(e) => Response::err("unknown", &e.code, e.detail),
      };
      let Ok(out) = serde_json::to_vec(&resp) else {
        continue;
      };
      if datagrams.send_datagram(out.into()).is_err() {
        break;
      }
    }
  });

  while let Ok((mut send, mut recv)) = conn.accept_bi().await {
    let st = state.clone();
    tokio::spawn(async move {
      let Ok(bytes) = recv.read_to_end(st.max_request_bytes).await else {
        return;
      };
      let resp = match parse_request(&bytes) {
        Ok(req) => dispatch(&st, &req, now()),
        Err(e) => Response::err("unknown", &e.code, e.detail),
      };
      let Ok(out) = serde_json::to_vec(&resp) else {
        return;
      };
      let _ = send.write_all(&out).await;
      let _ = send.finish();
    });
  }
  tracing::debug!(%remote, "connection closed");
}

// ---------------------------------------------------------------------- rand

fn random_key() -> [u8; 32] {
  let mut out = [0u8; 32];
  getrandom::fill(&mut out).expect("system random is available");
  out
}

fn random_hex(n_bytes: usize) -> String {
  let mut buf = vec![0u8; n_bytes];
  getrandom::fill(&mut buf).expect("system random is available");
  buf.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
  use super::*;
  use base64::engine::general_purpose::STANDARD as B64;
  use q2q_core::identity::{Ticket, keypair_from_seed, peer_id_of, sign_message};
  use q2q_punch::message::{HeartbeatRequest, JoinRequest, binding_mac, join_signing_bytes};
  use rustls::pki_types::CertificateDer;
  use std::sync::Arc;

  fn state(ttl: u64) -> State {
    State {
      bindings: Bindings::new(ttl),
      punch_id: "punch-1".into(),
      ttl,
      max_request_bytes: 64 * 1024,
    }
  }

  fn join_request(seed: u8, aud: &str, now: u64) -> JoinRequest {
    let (sk, _) = keypair_from_seed(&[seed; 32]);
    let pk = sk.verifying_key().to_bytes();
    let mut ticket = Ticket {
      v: 1,
      iss: peer_id_of(&sk),
      public_key: pk,
      iat: now,
      exp: now + 300,
      aud: audience(aud),
      tgt: None,
      ih: None,
      cid: Some(format!("cid-{seed}-{now}")),
      sig: [0u8; 64],
    };
    ticket.sign(&sk);
    let token = serde_json::to_string(&ticket.to_body()).unwrap();
    JoinRequest {
      join_token: token.clone(),
      public_key: B64.encode(pk),
      signature: B64.encode(sign_message(&sk, &join_signing_bytes(&token, now))),
      timestamp: now,
    }
  }

  /// `Ticket::aud` is an enum in core; build it through the public API.
  fn audience(aud: &str) -> q2q_core::identity::Audience {
    q2q_core::identity::Audience::from_wire(aud)
  }

  fn call(state: &State, method: &str, params: serde_json::Value) -> Response {
    let req = Request {
      v: 1,
      id: "r-1".into(),
      method: method.into(),
      params,
    };
    dispatch(state, &req, 1_000_000)
  }

  #[test]
  fn join_then_heartbeat_then_exit() {
    let now = 1_000_000;
    let st = state(300);

    let join = join_request(1, "punch-1", now);
    let resp = call(&st, "punch.join", serde_json::to_value(&join).unwrap());
    assert!(resp.error.is_none(), "{:?}", resp.error);
    let joined: JoinResponse = serde_json::from_value(resp.result.unwrap()).unwrap();

    let key: [u8; 32] = B64.decode(&joined.binding_key).unwrap().try_into().unwrap();
    let hb = HeartbeatRequest {
      binding_id: joined.binding_id.clone(),
      seq: 1,
      timestamp: now,
      generation: 0,
      mac: binding_mac(&key, &joined.binding_id, 1, now, 0),
    };
    let resp = call(&st, "punch.heartbeat", serde_json::to_value(&hb).unwrap());
    assert!(resp.error.is_none(), "{:?}", resp.error);

    let exit = ExitRequest {
      binding_id: joined.binding_id.clone(),
      seq: 2,
      timestamp: now,
      generation: 0,
      mac: binding_mac(&key, &joined.binding_id, 2, now, 0),
    };
    let resp = call(&st, "punch.exit", serde_json::to_value(&exit).unwrap());
    assert!(resp.error.is_none(), "{:?}", resp.error);
    assert_eq!(st.bindings.len(), 0);
  }

  #[test]
  fn a_replayed_join_is_refused() {
    let now = 1_000_000;
    let st = state(300);
    let join = join_request(2, "punch-1", now);

    let first = call(&st, "punch.join", serde_json::to_value(&join).unwrap());
    assert!(first.error.is_none());

    let second = call(&st, "punch.join", serde_json::to_value(&join).unwrap());
    assert_eq!(second.error.unwrap().code, "ticket_replayed");
  }

  #[test]
  fn a_ticket_for_another_punch_is_refused() {
    let now = 1_000_000;
    let st = state(300);
    let join = join_request(3, "punch-2", now);
    let resp = call(&st, "punch.join", serde_json::to_value(&join).unwrap());
    assert_eq!(resp.error.unwrap().code, "wrong_audience");
  }

  #[test]
  fn a_heartbeat_with_a_wrong_mac_is_refused() {
    let now = 1_000_000;
    let st = state(300);
    let join = join_request(4, "punch-1", now);
    let joined: JoinResponse = serde_json::from_value(
      call(&st, "punch.join", serde_json::to_value(&join).unwrap())
        .result
        .unwrap(),
    )
    .unwrap();

    let hb = HeartbeatRequest {
      binding_id: joined.binding_id.clone(),
      seq: 1,
      timestamp: now,
      generation: 0,
      mac: "00".repeat(32),
    };
    let resp = call(&st, "punch.heartbeat", serde_json::to_value(&hb).unwrap());
    assert_eq!(resp.error.unwrap().code, "bad_mac");
  }

  #[test]
  fn an_unknown_method_is_reported() {
    let st = state(300);
    let resp = call(&st, "punch.signal", serde_json::json!({}));
    assert_eq!(resp.error.unwrap().code, "unknown_method");
  }

  // ------------------------------------------------------- over a real socket

  fn test_client() -> quinn::Endpoint {
    // rustls will not guess a provider for us
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let crypto = rustls::ClientConfig::builder()
      .dangerous()
      .with_custom_certificate_verifier(Arc::new(AcceptAny))
      .with_no_client_auth();
    let cfg = quinn::crypto::rustls::QuicClientConfig::try_from(crypto).unwrap();
    let mut endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
    endpoint.set_default_client_config(quinn::ClientConfig::new(Arc::new(cfg)));
    endpoint
  }

  #[derive(Debug)]
  struct AcceptAny;

  impl rustls::client::danger::ServerCertVerifier for AcceptAny {
    fn verify_server_cert(
      &self,
      _end_entity: &CertificateDer<'_>,
      _intermediates: &[CertificateDer<'_>],
      _server_name: &rustls::pki_types::ServerName<'_>,
      _ocsp: &[u8],
      _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
      Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
      &self,
      _message: &[u8],
      _cert: &CertificateDer<'_>,
      _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
      Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
      &self,
      _message: &[u8],
      _cert: &CertificateDer<'_>,
      _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
      Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
      vec![rustls::SignatureScheme::ED25519]
    }
  }

  async fn rpc(conn: &quinn::Connection, req: &Request) -> Response {
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    send
      .write_all(&serde_json::to_vec(req).unwrap())
      .await
      .unwrap();
    let _ = send.finish();
    let bytes = recv.read_to_end(64 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
  }

  #[tokio::test]
  async fn join_and_heartbeat_over_quic() {
    let identity = crate::cert::PunchIdentity::generate("punch-1").unwrap();
    let bind: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let ep = endpoint(&identity, bind).unwrap();
    let addr = ep.local_addr().unwrap();

    let st = Arc::new(state(300));
    let server_state = st.clone();
    tokio::spawn(async move { serve(ep, server_state).await });

    let client = test_client();
    let conn = client
      .connect(addr, "punch-1")
      .unwrap()
      .await
      .expect("handshake");

    // a real socket means a real clock: the server checks the timestamp
    let now = q2q_punch::binding::now();
    let join = join_request(5, "punch-1", now);
    let resp = rpc(
      &conn,
      &Request {
        v: 1,
        id: "r-1".into(),
        method: "punch.join".into(),
        params: serde_json::to_value(&join).unwrap(),
      },
    )
    .await;
    assert!(resp.error.is_none(), "{:?}", resp.error);
    let joined: JoinResponse = serde_json::from_value(resp.result.unwrap()).unwrap();

    // heartbeat rides a datagram: no stream, no ordering
    let key: [u8; 32] = B64.decode(&joined.binding_key).unwrap().try_into().unwrap();
    let hb = Request {
      v: 1,
      id: "r-2".into(),
      method: "punch.heartbeat".into(),
      params: serde_json::to_value(&HeartbeatRequest {
        binding_id: joined.binding_id.clone(),
        seq: 1,
        timestamp: now,
        generation: 0,
        mac: binding_mac(&key, &joined.binding_id, 1, now, 0),
      })
      .unwrap(),
    };
    conn
      .send_datagram(serde_json::to_vec(&hb).unwrap().into())
      .unwrap();

    let bytes = conn.read_datagram().await.unwrap();
    let resp: Response = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(resp.id, "r-2");
    assert!(resp.error.is_none(), "{:?}", resp.error);

    conn.close(0u32.into(), b"done");
    client.wait_idle().await;
  }

  #[tokio::test]
  async fn a_second_join_replaces_the_first_binding() {
    let st = state(300);
    let now = 1_000_000;

    let first = call(
      &st,
      "punch.join",
      serde_json::to_value(join_request(6, "punch-1", now)).unwrap(),
    );
    assert!(first.error.is_none());
    let joined: JoinResponse = serde_json::from_value(first.result.unwrap()).unwrap();

    // same peer, fresh cid — the old binding id must stop working
    let (sk, _) = keypair_from_seed(&[6; 32]);
    let pk = sk.verifying_key().to_bytes();
    let mut ticket = Ticket {
      v: 1,
      iss: peer_id_of(&sk),
      public_key: pk,
      iat: now,
      exp: now + 300,
      aud: audience("punch-1"),
      tgt: None,
      ih: None,
      cid: Some("cid-second".into()),
      sig: [0u8; 64],
    };
    ticket.sign(&sk);
    let token = serde_json::to_string(&ticket.to_body()).unwrap();
    let join = JoinRequest {
      join_token: token.clone(),
      public_key: B64.encode(pk),
      signature: B64.encode(sign_message(&sk, &join_signing_bytes(&token, now))),
      timestamp: now,
    };
    let resp = call(&st, "punch.join", serde_json::to_value(&join).unwrap());
    assert!(resp.error.is_none());

    let stale = call(
      &st,
      "punch.heartbeat",
      serde_json::to_value(&HeartbeatRequest {
        binding_id: joined.binding_id,
        seq: 1,
        timestamp: now,
        generation: 0,
        mac: "00".repeat(32),
      })
      .unwrap(),
    );
    assert_eq!(stale.error.unwrap().code, "unknown_binding");
  }
}
