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
use crate::signalling::{Caller, ChannelSink, Outcome, Sinks, handle_signal};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use q2q_core::identity::PeerId;
use q2q_punch::binding::{Binding, Bindings, UnixSeconds, now};
use q2q_punch::message::{
  CLOCK_TOLERANCE, ExitRequest, HeartbeatRequest, HeartbeatResponse, JoinRequest, JoinResponse,
  Method, Request, Response, parse_request, verify_binding_mac,
};
use q2q_punch::signal::{Connections, SignalRequest};
use q2q_punch::transport::PunchFrame;
use q2q_ratelimit::Limiter;
use quinn::{Connection, Endpoint};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::accept_async_with_config;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

pub struct State {
  pub bindings: Bindings,
  pub connections: Connections,
  /// Channels to the peers themselves. Reachability lives here, not in a table.
  pub sinks: Arc<Sinks>,
  pub limits: Limiter,
  /// Tickets must name this exact server, so one leak cannot be replayed
  /// against a different Punch node.
  pub punch_id: String,
  pub ttl: UnixSeconds,
  pub max_request_bytes: usize,
}

// ------------------------------------------------------------------- dispatch

/// Handle one request. Pure apart from the binding table and the rng, so it can
/// be tested without a socket.
/// Handle one request. The reply is only part of it: a join binds a standing
/// channel and a signal may owe a message to the other side, so the transport
/// gets an `Outcome` rather than just a `Response`.
///
/// Pure apart from the tables and the rng, so it can be tested without a socket.
pub fn dispatch(state: &State, req: &Request, now: UnixSeconds, caller: &Caller) -> Outcome {
  let response = |r: Response| Outcome {
    response: r,
    forwards: Vec::new(),
    joined: None,
    ephemeral: None,
  };

  let Some(method) = Method::parse(&req.method) else {
    return response(Response::err(
      &req.id,
      "unknown_method",
      Some(req.method.clone()),
    ));
  };

  match method {
    Method::Join => match serde_json::from_value::<JoinRequest>(req.params.clone()) {
      Ok(join) => handle_join(state, req, &join, now),
      Err(e) => response(Response::err(
        &req.id,
        "malformed_params",
        Some(e.to_string()),
      )),
    },
    Method::Signal => {
      let Ok(sig) = serde_json::from_value::<SignalRequest>(req.params.clone()) else {
        return response(Response::err(&req.id, "malformed_params", None));
      };
      handle_signal(state, req, &sig, caller, now)
    }
    Method::Heartbeat => match serde_json::from_value::<HeartbeatRequest>(req.params.clone()) {
      Ok(hb) => response(handle_heartbeat(state, req, &hb, now)),
      Err(e) => response(Response::err(
        &req.id,
        "malformed_params",
        Some(e.to_string()),
      )),
    },
    Method::Exit => match serde_json::from_value::<ExitRequest>(req.params.clone()) {
      Ok(ex) => response(handle_exit(state, req, &ex, now)),
      Err(e) => response(Response::err(
        &req.id,
        "malformed_params",
        Some(e.to_string()),
      )),
    },
  }
}

fn handle_join(state: &State, req: &Request, join: &JoinRequest, now: UnixSeconds) -> Outcome {
  let plain = |r: Response| Outcome {
    response: r,
    forwards: Vec::new(),
    joined: None,
    ephemeral: None,
  };

  let ticket = match q2q_punch::message::verify_join(join, &state.punch_id, now) {
    Ok(t) => t,
    Err(e) => {
      let code = e.code();
      return plain(Response::err(&req.id, code, Some(e.to_string())));
    }
  };

  // replay protection rests entirely on the one-shot ticket id: join has no
  // challenge, so a second join with the same cid must be refused
  let Some(cid) = ticket.cid.clone() else {
    return plain(Response::err(
      &req.id,
      "malformed_ticket",
      Some("join ticket needs a cid".into()),
    ));
  };
  if !state.bindings.spend(&cid, now, ticket.exp) {
    return plain(Response::err(&req.id, "ticket_replayed", None));
  }

  // both come from the system random source; without them there is no binding
  let (binding_id, key) = match (random_hex(16), random_key()) {
    (Ok(id), Ok(key)) => (id, key),
    (Err(e), _) | (_, Err(e)) => {
      tracing::error!("could not mint a binding key: {e}");
      return plain(Response::err(
        &req.id,
        "internal_error",
        Some(e.to_string()),
      ));
    }
  };

  let binding = Binding {
    binding_id,
    peer_id: q2q_core::identity::PeerId::unchecked(&ticket.iss),
    key,
    expires_at: now + state.ttl,
    last_seq: 0,
    generation: 0,
  };

  state.bindings.insert(binding.clone());
  tracing::info!(peer = %binding.peer_id.as_str(), punch = %state.punch_id, "binding established");

  Outcome {
    response: Response::ok(
      &req.id,
      &JoinResponse {
        binding_id: binding.binding_id,
        binding_key: B64.encode(binding.key),
        expires_at: binding.expires_at,
      },
    ),
    forwards: Vec::new(),
    // the caller is now reachable: this channel is its standing one
    joined: Some(binding.peer_id),
    ephemeral: None,
  }
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

/// Rate limit first, then dispatch. Identities are free to mint, so the limit is
/// charged to the network the request came from — see `limits::classify`.
fn handle_guarded(state: &State, req: Request, caller: &Caller, ip: std::net::IpAddr) -> Outcome {
  let verdict = crate::limits::charge(&state.limits, &req, ip, crate::limits::now_ms());
  if !verdict.allowed {
    let wait = verdict.retry_after_ms.unwrap_or(1_000);
    tracing::debug!(id = %req.id, retry_after_ms = wait, "rate limited");
    return Outcome {
      response: Response::err(
        &req.id,
        "rate_limited",
        Some(format!("retry after {wait} ms")),
      ),
      forwards: Vec::new(),
      joined: None,
      ephemeral: None,
    };
  }
  dispatch(state, &req, now(), caller)
}

/// Per-connection identity and signal routes shared by every transport
/// adapter. The adapter owns framing and socket I/O; request authorization,
/// dispatch, and channel lifecycle stay here.
struct PunchChannel {
  sink: ChannelSink,
  authenticated_peer: Mutex<Option<PeerId>>,
  ephemeral_ids: Mutex<Vec<String>>,
}

impl PunchChannel {
  fn new(sink: ChannelSink) -> Self {
    Self {
      sink,
      authenticated_peer: Mutex::new(None),
      ephemeral_ids: Mutex::new(Vec::new()),
    }
  }

  async fn dispatch(&self, state: &State, req: Request, ip: std::net::IpAddr) -> Response {
    let ephemeral_peer = serde_json::from_value::<SignalRequest>(req.params.clone())
      .ok()
      .filter(|sig| {
        self.ephemeral_ids.lock().contains(&sig.connection_id)
          && state
            .sinks
            .ephemeral_is_current(&sig.connection_id, &self.sink)
      })
      .and_then(|sig| state.connections.get(&sig.connection_id))
      .map(|connection| connection.a);
    let caller = Caller {
      peer: self.authenticated_peer.lock().clone().or(ephemeral_peer),
    };
    let outcome = handle_guarded(state, req, &caller, ip);

    if let Some(peer) = &outcome.joined {
      state.sinks.put_standing(peer, self.sink.clone());
      *self.authenticated_peer.lock() = Some(peer.clone());
    }
    if let Some(connection_id) = &outcome.ephemeral {
      state.sinks.put_ephemeral(connection_id, self.sink.clone());
      self.ephemeral_ids.lock().push(connection_id.clone());
    }

    crate::signalling::deliver(&state.sinks, outcome.forwards).await;
    outcome.response
  }

  fn cleanup(&self, state: &State) {
    if let Some(peer) = self.authenticated_peer.lock().as_ref() {
      state.sinks.drop_standing_if_current(peer, &self.sink);
    }
    for connection_id in self.ephemeral_ids.lock().iter() {
      state
        .sinks
        .drop_ephemeral_if_current(connection_id, &self.sink);
    }
  }
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

pub async fn serve_websocket(
  listener: TcpListener,
  acceptor: TlsAcceptor,
  state: Arc<State>,
) -> std::io::Result<()> {
  loop {
    let (stream, remote) = listener.accept().await?;
    let st = state.clone();
    let tls = acceptor.clone();
    tokio::spawn(async move {
      let tls_stream =
        match tokio::time::timeout(std::time::Duration::from_secs(10), tls.accept(stream)).await {
          Ok(Ok(stream)) => stream,
          Ok(Err(error)) => {
            tracing::debug!(%remote, %error, "WebSocket TLS handshake failed");
            return;
          }
          Err(_) => {
            tracing::debug!(%remote, "WebSocket TLS handshake timed out");
            return;
          }
        };
      let ws_config = WebSocketConfig::default()
        .max_message_size(Some(st.max_request_bytes))
        .max_frame_size(Some(st.max_request_bytes));
      match accept_async_with_config(tls_stream, Some(ws_config)).await {
        Ok(socket) => handle_websocket(socket, st, remote).await,
        Err(error) => tracing::debug!(%remote, %error, "WebSocket handshake failed"),
      }
    });
  }
}

async fn handle_websocket<S>(
  socket: tokio_tungstenite::WebSocketStream<S>,
  state: Arc<State>,
  remote: SocketAddr,
) where
  S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
  let ip = remote.ip();
  let (mut writer, mut reader) = socket.split();
  let (mine, mut outgoing) = ChannelSink::channel(64);
  let channel = PunchChannel::new(mine.clone());
  tracing::debug!(%remote, "WebSocket Punch connection accepted");

  loop {
    tokio::select! {
      pending = outgoing.recv() => {
        let Some(pending) = pending else { break; };
        let delivered = match serde_json::to_vec(&PunchFrame::Signal(pending.signal)) {
            Ok(bytes) => writer
              .send(Message::Binary(bytes.into()))
              .await
              .map_err(|error| error.to_string()),
            Err(error) => Err(error.to_string()),
        };
        let ok = delivered.is_ok();
        let _ = pending.delivered.send(ok);
        if let Err(error) = delivered {
          tracing::debug!(%remote, %error, "WebSocket signal delivery failed");
          break;
        }
      }
      incoming = reader.next() => {
        let Some(incoming) = incoming else { break; };
        let message = match incoming {
          Ok(message) => message,
          Err(error) => {
            tracing::debug!(%remote, %error, "WebSocket read failed");
            break;
          }
        };
        match message {
          Message::Binary(bytes) => {
            let frame = match serde_json::from_slice::<PunchFrame>(&bytes) {
              Ok(frame) => frame,
              Err(error) => {
                tracing::debug!(%remote, %error, "invalid WebSocket Punch frame");
                break;
              }
            };
            let PunchFrame::Request(req) = frame else {
              tracing::debug!(%remote, "client sent a non-request Punch frame");
              break;
            };
            let response = channel.dispatch(&state, req, ip).await;
            let bytes = match serde_json::to_vec(&PunchFrame::Response(response)) {
              Ok(bytes) => bytes,
              Err(error) => {
                tracing::error!(%error, "could not encode WebSocket Punch response");
                break;
              }
            };
            if let Err(error) = writer.send(Message::Binary(bytes.into())).await {
              tracing::debug!(%remote, %error, "WebSocket write failed");
              break;
            }
          }
          Message::Ping(bytes) => {
            if writer.send(Message::Pong(bytes)).await.is_err() {
              break;
            }
          }
          Message::Close(_) => break,
          Message::Pong(_) => {}
          Message::Text(_) | Message::Frame(_) => {
            tracing::debug!(%remote, "unsupported WebSocket frame");
            break;
          }
        }
      }
    }
  }

  channel.cleanup(&state);
  let _ = writer.send(Message::Close(None)).await;
  tracing::debug!(%remote, "WebSocket Punch connection closed");
}

async fn handle_connection(conn: Connection, state: Arc<State>) {
  let remote = conn.remote_address();
  let ip = remote.ip();
  tracing::debug!(%remote, "connection accepted");

  // Routes keep only this transport-neutral handle. This writer task owns the
  // QUIC-specific stream mechanics; the WebSocket adapter owns frame writes.
  let (mine, mut outgoing) = ChannelSink::channel(64);
  let writer = conn.clone();
  tokio::spawn(async move {
    while let Some(pending) = outgoing.recv().await {
      let delivered = match writer.open_uni().await {
        Ok(mut send) => {
          if let Ok(bytes) = serde_json::to_vec(&pending.signal)
            && send.write_all(&bytes).await.is_ok()
          {
            send.finish().is_ok()
          } else {
            false
          }
        }
        Err(_) => false,
      };
      let _ = pending.delivered.send(delivered);
      if !delivered {
        break;
      }
    }
  });

  // heartbeats: loss is tolerated, so they do not need a stream
  let datagrams = conn.clone();
  let dg_state = state.clone();
  tokio::spawn(async move {
    while let Ok(bytes) = datagrams.read_datagram().await {
      let outcome = match parse_request(&bytes) {
        Ok(req) => handle_guarded(&dg_state, req, &Caller::default(), ip),
        Err(e) => Outcome {
          response: Response::err("unknown", &e.code, e.detail),
          forwards: Vec::new(),
          joined: None,
          ephemeral: None,
        },
      };
      let Ok(out) = serde_json::to_vec(&outcome.response) else {
        continue;
      };
      if datagrams.send_datagram(out.into()).is_err() {
        break;
      }
    }
  });

  // The shared dispatcher owns identity and routes; QUIC tasks only decode
  // the request stream and encode its response.
  let channel = Arc::new(PunchChannel::new(mine.clone()));

  while let Ok((mut send, mut recv)) = conn.accept_bi().await {
    let st = state.clone();
    let channel = channel.clone();
    tokio::spawn(async move {
      let Ok(bytes) = recv.read_to_end(st.max_request_bytes).await else {
        return;
      };
      let response = match parse_request(&bytes) {
        Ok(req) => channel.dispatch(&st, req, ip).await,
        Err(e) => Response::err("unknown", &e.code, e.detail),
      };
      let out = match serde_json::to_vec(&response) {
        Ok(o) => o,
        Err(_) => return,
      };
      let _ = send.write_all(&out).await;
      let _ = send.finish();
    });
  }

  // a closed standing channel means an unreachable peer, so drop it here
  channel.cleanup(&state);
  tracing::debug!(%remote, "connection closed");
}

// ---------------------------------------------------------------------- rand

/// The system random source gave us nothing. Nothing about the request was
/// wrong, so this is a 5xx rather than a rejection of the client.
#[derive(Debug)]
pub struct RandomError;

impl std::fmt::Display for RandomError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "system random is unavailable")
  }
}

impl std::error::Error for RandomError {}

fn random_key() -> Result<[u8; 32], RandomError> {
  let mut out = [0u8; 32];
  getrandom::fill(&mut out).map_err(|_| RandomError)?;
  Ok(out)
}

fn random_hex(n_bytes: usize) -> Result<String, RandomError> {
  let mut buf = vec![0u8; n_bytes];
  getrandom::fill(&mut buf).map_err(|_| RandomError)?;
  Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
  use super::*;
  use base64::engine::general_purpose::STANDARD as B64;
  use q2q_core::identity::{
    Audience, TICKET_VERSION, Ticket, keypair_from_seed, peer_id_of, sign_message,
  };
  use q2q_punch::message::{
    HeartbeatRequest, JoinRequest, SignalResponse, binding_mac, join_signing_bytes,
  };
  use q2q_punch::signal::SignalType;
  use rustls::pki_types::CertificateDer;
  use std::sync::Arc;

  fn state(ttl: u64) -> State {
    state_with_limits(
      ttl,
      crate::limits::limiter(crate::limits::Rates::default(), 1),
    )
  }

  /// A small bucket makes the limit observable in a test.
  fn state_with_limits(ttl: u64, limits: Limiter) -> State {
    State {
      bindings: Bindings::new(ttl),
      connections: Connections::new(ttl),
      sinks: Arc::new(Sinks::default()),
      limits,
      punch_id: "punch-1".into(),
      ttl,
      max_request_bytes: 64 * 1024,
    }
  }

  fn join_request(seed: u8, aud: &str, now: u64) -> JoinRequest {
    let (sk, _) = keypair_from_seed(&[seed; 32]);
    let pk = sk.verifying_key().to_bytes();
    let mut ticket = Ticket {
      v: TICKET_VERSION,
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

  /// Punch ids carry a `punch:` prefix on the wire, so build the whole thing
  /// here rather than at each caller.
  fn audience(aud: &str) -> String {
    Audience::Punch(aud.to_string()).to_wire()
  }

  fn call_with(state: &State, method: &str, params: serde_json::Value, caller: Caller) -> Outcome {
    let req = Request {
      v: 1,
      id: "r-1".into(),
      method: method.into(),
      params,
    };
    dispatch(state, &req, 1_000_000, &caller)
  }

  fn call(state: &State, method: &str, params: serde_json::Value) -> Response {
    call_with(state, method, params, Caller::default()).response
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
    let resp = call(&st, "punch.nope", serde_json::json!({}));
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
  async fn websocket_adapter_dispatches_and_forwards_typed_signals() {
    let st = Arc::new(state(60));
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let server_socket = tokio_tungstenite::WebSocketStream::from_raw_socket(
      server_io,
      tokio_tungstenite::tungstenite::protocol::Role::Server,
      None,
    )
    .await;
    let mut client_socket = tokio_tungstenite::WebSocketStream::from_raw_socket(
      client_io,
      tokio_tungstenite::tungstenite::protocol::Role::Client,
      None,
    )
    .await;
    let server_state = st.clone();
    let handler = tokio::spawn(async move {
      handle_websocket(
        server_socket,
        server_state,
        "127.0.0.1:41000".parse().unwrap(),
      )
      .await;
    });

    let peer = peer(11);
    let now = q2q_punch::binding::now();
    let join = join_request(11, "punch-1", now);
    let request = Request {
      v: 1,
      id: "ws-join".into(),
      method: "punch.join".into(),
      params: serde_json::to_value(join).unwrap(),
    };
    client_socket
      .send(tokio_tungstenite::tungstenite::Message::Binary(
        serde_json::to_vec(&PunchFrame::Request(request))
          .unwrap()
          .into(),
      ))
      .await
      .unwrap();

    let message = tokio::time::timeout(std::time::Duration::from_secs(2), client_socket.next())
      .await
      .unwrap()
      .unwrap()
      .unwrap();
    let tokio_tungstenite::tungstenite::Message::Binary(bytes) = message else {
      panic!("Punch WebSocket response must be binary");
    };
    let PunchFrame::Response(response) = serde_json::from_slice(&bytes).unwrap() else {
      panic!("server should answer the Punch request");
    };
    assert_eq!(response.id, "ws-join");
    assert!(response.error.is_none(), "{:?}", response.error);
    assert!(st.sinks.has_standing(&peer));

    let (server_io_a, client_io_a) = tokio::io::duplex(64 * 1024);
    let server_socket_a = tokio_tungstenite::WebSocketStream::from_raw_socket(
      server_io_a,
      tokio_tungstenite::tungstenite::protocol::Role::Server,
      None,
    )
    .await;
    let mut client_socket_a = tokio_tungstenite::WebSocketStream::from_raw_socket(
      client_io_a,
      tokio_tungstenite::tungstenite::protocol::Role::Client,
      None,
    )
    .await;
    let server_state_a = st.clone();
    let handler_a = tokio::spawn(async move {
      handle_websocket(
        server_socket_a,
        server_state_a,
        "127.0.0.1:41001".parse().unwrap(),
      )
      .await;
    });
    let mut connect = signal(SignalType::Connect, "ws-connect");
    connect.connect_token = Some(connect_token(12, "punch-1", &peer, "ws-connect", now));
    client_socket_a
      .send(tokio_tungstenite::tungstenite::Message::Binary(
        serde_json::to_vec(&PunchFrame::Request(Request {
          v: 1,
          id: "ws-connect-request".into(),
          method: "punch.signal".into(),
          params: serde_json::to_value(connect).unwrap(),
        }))
        .unwrap()
        .into(),
      ))
      .await
      .unwrap();

    let forwarded = tokio::time::timeout(std::time::Duration::from_secs(2), client_socket.next())
      .await
      .unwrap()
      .unwrap()
      .unwrap();
    let tokio_tungstenite::tungstenite::Message::Binary(bytes) = forwarded else {
      panic!("forwarded Punch signals must be binary");
    };
    let PunchFrame::Signal(forwarded) = serde_json::from_slice(&bytes).unwrap() else {
      panic!("target should receive a signal frame");
    };
    assert_eq!(forwarded.kind, SignalType::Connect);
    assert_eq!(forwarded.connection_id, "ws-connect");

    let response_a =
      tokio::time::timeout(std::time::Duration::from_secs(2), client_socket_a.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let tokio_tungstenite::tungstenite::Message::Binary(bytes) = response_a else {
      panic!("Punch response must be binary");
    };
    let PunchFrame::Response(response_a) = serde_json::from_slice(&bytes).unwrap() else {
      panic!("initiator should receive an RPC response");
    };
    assert_eq!(response_a.id, "ws-connect-request");
    assert!(response_a.error.is_none(), "{:?}", response_a.error);

    client_socket
      .send(tokio_tungstenite::tungstenite::Message::Close(None))
      .await
      .unwrap();
    client_socket_a
      .send(tokio_tungstenite::tungstenite::Message::Close(None))
      .await
      .unwrap();
    handler.await.unwrap();
    handler_a.await.unwrap();
    assert!(!st.sinks.has_standing(&peer));
  }

  #[tokio::test]
  async fn a_second_join_from_the_same_network_is_rate_limited() {
    let identity = crate::cert::PunchIdentity::generate("punch-1").unwrap();
    let bind: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let ep = endpoint(&identity, bind).unwrap();
    let addr = ep.local_addr().unwrap();

    // one join per minute per network
    let st = Arc::new(state_with_limits(
      60,
      crate::limits::limiter(
        crate::limits::Rates {
          join: 1,
          ..Default::default()
        },
        1,
      ),
    ));
    let server_state = st.clone();
    tokio::spawn(async move { serve(ep, server_state).await });

    let client = test_client();
    let now = q2q_punch::binding::now();

    // two connections, but both from 127.0.0.1 — one bucket
    let first = client.connect(addr, "punch-1").unwrap().await.unwrap();
    let second = client.connect(addr, "punch-1").unwrap().await.unwrap();

    let joined = rpc(
      &first,
      &Request {
        v: 1,
        id: "r-1".into(),
        method: "punch.join".into(),
        params: serde_json::to_value(join_request(7, "punch-1", now)).unwrap(),
      },
    )
    .await;
    assert!(joined.error.is_none(), "{:?}", joined.error);

    let denied = rpc(
      &second,
      &Request {
        v: 1,
        id: "r-2".into(),
        method: "punch.join".into(),
        params: serde_json::to_value(join_request(8, "punch-1", now)).unwrap(),
      },
    )
    .await;
    let error = denied.error.expect("the second join must be refused");
    assert_eq!(error.code, "rate_limited");
    assert!(error.detail.unwrap_or_default().contains("retry after"));

    first.close(0u32.into(), b"done");
    second.close(0u32.into(), b"done");
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
      v: TICKET_VERSION,
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

  // ------------------------------------------------------------ signalling

  /// A connect ticket: names this Punch Server, the target, and the content.
  fn connect_token(seed: u8, aud: &str, tgt: &PeerId, cid: &str, now: u64) -> String {
    let (sk, _) = keypair_from_seed(&[seed; 32]);
    let mut t = Ticket {
      v: TICKET_VERSION,
      iss: peer_id_of(&sk),
      public_key: sk.verifying_key().to_bytes(),
      iat: now,
      exp: now + 60,
      aud: audience(aud),
      tgt: Some(tgt.clone()),
      ih: Some([7u8; 32]),
      cid: Some(cid.into()),
      sig: [0u8; 64],
    };
    t.sign(&sk);
    serde_json::to_string(&t.to_body()).unwrap()
  }

  fn signal(kind: SignalType, cid: &str) -> SignalRequest {
    SignalRequest {
      kind,
      connection_id: cid.into(),
      connect_token: None,
      sdp: None,
      candidate: None,
      reason: None,
      accept_token: None,
      signal_key: None,
    }
  }

  /// Collect what the server pushes onto this channel.
  fn collect(conn: &quinn::Connection) -> Arc<Mutex<Vec<Vec<u8>>>> {
    let seen: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let c = conn.clone();
    tokio::spawn(async move {
      while let Ok(mut stream) = c.accept_uni().await {
        if let Ok(bytes) = stream.read_to_end(64 * 1024).await {
          sink.lock().push(bytes);
        }
      }
    });
    seen
  }

  async fn send_signal(conn: &quinn::Connection, sig: &SignalRequest) -> Response {
    rpc(
      conn,
      &Request {
        v: 1,
        id: format!("sig-{}", sig.kind.name()),
        method: "punch.signal".into(),
        params: serde_json::to_value(sig).unwrap(),
      },
    )
    .await
  }

  #[test]
  fn a_target_without_a_standing_channel_is_unreachable() {
    let st = state(60);
    let now = 1_000_000;
    let b = peer(2);

    // reachability is the channel, not a table: nobody joined, so nobody is home
    let mut sig = signal(SignalType::Connect, "cid-a");
    sig.connect_token = Some(connect_token(1, "punch-1", &b, "cid-a", now));
    let resp = call(&st, "punch.signal", serde_json::to_value(&sig).unwrap());
    assert_eq!(resp.error.unwrap().code, "peer_offline");
  }

  #[tokio::test]
  async fn signalling_reaches_the_target_over_quic() {
    let identity = crate::cert::PunchIdentity::generate("punch-1").unwrap();
    let bind: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let ep = endpoint(&identity, bind).unwrap();
    let addr = ep.local_addr().unwrap();

    let st = Arc::new(state(60));
    let server_state = st.clone();
    tokio::spawn(async move { serve(ep, server_state).await });

    let client = test_client();

    // B joins: from here on it is reachable
    let b_conn = client.connect(addr, "punch-1").unwrap().await.unwrap();
    let (sk_b, _) = keypair_from_seed(&[2u8; 32]);
    let b_peer = peer_id_of(&sk_b);
    let now = q2q_punch::binding::now();
    let join = join_request(2, "punch-1", now);
    let joined: JoinResponse = serde_json::from_value(
      rpc(
        &b_conn,
        &Request {
          v: 1,
          id: "j".into(),
          method: "punch.join".into(),
          params: serde_json::to_value(&join).unwrap(),
        },
      )
      .await
      .result
      .unwrap(),
    )
    .unwrap();
    assert!(!joined.binding_id.is_empty());
    let b_inbox = collect(&b_conn);

    // A opens a connection of its own
    let a_conn = client.connect(addr, "punch-1").unwrap().await.unwrap();
    let a_inbox = collect(&a_conn);

    let mut sig = signal(SignalType::Connect, "cid-1");
    sig.connect_token = Some(connect_token(1, "punch-1", &b_peer, "cid-1", now));
    let resp = send_signal(&a_conn, &sig).await;
    assert!(resp.error.is_none(), "{:?}", resp.error);

    // B has to be told before A offers anything
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    {
      let inbox = b_inbox.lock();
      assert_eq!(inbox.len(), 1, "B should have received the connect");
      let got: SignalRequest = serde_json::from_slice(&inbox[0]).unwrap();
      assert_eq!(got.kind, SignalType::Connect);
    }

    // an offer before consent is refused
    let mut early = signal(SignalType::Offer, "cid-1");
    early.connect_token = Some(connect_token(1, "punch-1", &b_peer, "cid-1", now));
    early.sdp = Some("v=0".into());
    let resp = send_signal(&a_conn, &early).await;
    assert_eq!(resp.error.unwrap().code, "not_accepted");

    // B accepts — on its own standing channel, which is how it is identified
    let accept = signal(SignalType::Accept, "cid-1");
    let resp = send_signal(&b_conn, &accept).await;
    assert!(resp.error.is_none(), "{:?}", resp.error);

    // now the offer goes through, and mints the key
    let resp = send_signal(&a_conn, &early).await;
    assert!(resp.error.is_none(), "{:?}", resp.error);
    let out: SignalResponse = serde_json::from_value(resp.result.unwrap()).unwrap();
    assert!(out.signal_key.is_some());

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    {
      let inbox = b_inbox.lock();
      assert_eq!(inbox.len(), 2, "B gets the connect and the offer");
      let got: SignalRequest = serde_json::from_slice(&inbox[1]).unwrap();
      assert_eq!(got.kind, SignalType::Offer);
    }
    {
      // the accept itself went the other way, to A
      let inbox = a_inbox.lock();
      assert_eq!(inbox.len(), 1);
      let got: SignalRequest = serde_json::from_slice(&inbox[0]).unwrap();
      assert_eq!(got.kind, SignalType::Accept);
    }

    // B answers, and it lands on A's ephemeral channel
    let mut answer = signal(SignalType::Answer, "cid-1");
    answer.sdp = Some("v=0 answer".into());
    let resp = send_signal(&b_conn, &answer).await;
    assert!(resp.error.is_none(), "{:?}", resp.error);

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let inbox = a_inbox.lock();
    assert_eq!(inbox.len(), 2, "A gets the accept, then the answer");
    let got: SignalRequest = serde_json::from_slice(&inbox[1]).unwrap();
    assert_eq!(got.kind, SignalType::Answer);
  }

  #[test]
  fn an_answer_while_a_is_away_is_held_for_the_resume() {
    let st = state(60);
    let now = 1_000_000;
    let a = peer(1);
    let b = peer(2);

    // walk the connection up to the point where A would have offered
    let ticket = connect_token(1, "punch-1", &b, "cid-2", now);
    let body: q2q_core::identity::TicketBody = serde_json::from_str(&ticket).unwrap();
    st.connections.open(&body, &b, true, now).unwrap();
    st.connections.accept("cid-2", &b, now).unwrap();
    st.connections.offer("cid-2", &a, "sdp", now).unwrap();

    // B answers; A's ephemeral channel is gone, so it must be kept, not dropped
    let mut answer = signal(SignalType::Answer, "cid-2");
    answer.sdp = Some("v=0 answer".into());
    let outcome = call_with(
      &st,
      "punch.signal",
      serde_json::to_value(&answer).unwrap(),
      Caller { peer: Some(b) },
    );
    assert!(
      outcome.response.error.is_none(),
      "{:?}",
      outcome.response.error
    );
    assert!(
      outcome.forwards.is_empty(),
      "nothing to send while A is away"
    );

    let held = st.connections.get("cid-2").unwrap();
    assert_eq!(held.pending.len(), 1);
    assert!(held.pending[0].contains("answer"));
  }

  #[test]
  fn reject_reaches_initiator_after_removing_connection() {
    let st = state(60);
    let b = peer(2);
    let ticket = connect_token(1, "punch-1", &b, "cid-reject", 1_000_000);
    let body = serde_json::from_str(&ticket).unwrap();
    st.connections.open(&body, &b, true, 1_000_000).unwrap();
    let mut reject = signal(SignalType::Reject, "cid-reject");
    reject.reason = Some(q2q_punch::signal::RejectReason::Busy);
    let outcome = call_with(
      &st,
      "punch.signal",
      serde_json::to_value(&reject).unwrap(),
      Caller { peer: Some(b) },
    );
    assert!(outcome.response.error.is_none());
    assert!(st.connections.get("cid-reject").is_none());
    assert_eq!(outcome.forwards.len(), 1);
    assert!(
      matches!(&outcome.forwards[0].to, crate::signalling::Recipient::Initiator(id) if id == "cid-reject")
    );
    assert_eq!(outcome.forwards[0].signal.reason, reject.reason);
  }

  fn peer(seed: u8) -> PeerId {
    let (sk, _) = keypair_from_seed(&[seed; 32]);
    peer_id_of(&sk)
  }
}
