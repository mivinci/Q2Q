//! QUIC transport for the binding RPCs.
//!
//! Binding RPCs, including heartbeats, use reliable bidirectional streams.
//! Legacy heartbeat datagrams are still accepted. QUIC and TLS WebSocket share
//! the authenticated channel dispatcher and routing lifecycle.

use crate::cert::{CertError, PunchIdentity};
use crate::signalling::{Caller, ChannelSink, Outcome, Sinks, handle_signal};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use q2q_primitives::identity::PeerId;
use q2q_punch::binding::{Binding, Bindings, UnixSeconds, now};
use q2q_punch::message::{
  CLOCK_TOLERANCE, ExitRequest, HeartbeatRequest, HeartbeatResponse, JoinRequest, JoinResponse,
  Method, Request, Response, parse_request, validate_request_version, verify_binding_mac,
};
use q2q_punch::signal::{Connections, SignalRequest};
use q2q_punch::transport::PunchFrame;
use q2q_ratelimit::Limiter;
use quinn::{Connection, Endpoint};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, mpsc, oneshot};
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::accept_async_with_config;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

pub struct State {
  pub bindings: Bindings,
  pub connections: Connections,
  /// Routes to peers with live authenticated bindings and to ephemeral channels.
  pub sinks: Arc<Sinks>,
  pub limits: Limiter,
  /// Tickets must name this exact server, so one leak cannot be replayed
  /// against a different Punch node.
  pub punch_id: String,
  pub ttl: UnixSeconds,
  pub max_request_bytes: usize,
  pub quic_slots: Arc<Semaphore>,
  pub websocket_slots: Arc<Semaphore>,
}

const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const AUTH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
pub const CONNECTION_LIMIT: usize = 1024;
const CONNECTIONS_PER_IP: usize = 32;
const WEBSOCKET_WRITE_QUEUE: usize = 64;

#[derive(Clone)]
struct PerIpConnections(Arc<Mutex<HashMap<IpAddr, usize>>>);

impl PerIpConnections {
  fn acquire(&self, ip: IpAddr) -> Option<PerIpConnectionGuard> {
    let mut counts = self.0.lock();
    let count = counts.entry(ip).or_default();
    if *count >= CONNECTIONS_PER_IP {
      return None;
    }
    *count += 1;
    Some(PerIpConnectionGuard {
      counts: self.clone(),
      ip,
    })
  }
}

struct PerIpConnectionGuard {
  counts: PerIpConnections,
  ip: IpAddr,
}

impl Drop for PerIpConnectionGuard {
  fn drop(&mut self) {
    let mut counts = self.counts.0.lock();
    if let Some(count) = counts.get_mut(&self.ip) {
      *count -= 1;
      if *count == 0 {
        counts.remove(&self.ip);
      }
    }
  }
}

enum WebSocketWrite {
  Binary(Vec<u8>),
  Pong(Vec<u8>),
  Close,
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
    peer_id: q2q_primitives::identity::PeerId::unchecked(&ticket.iss),
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
        binding_id: binding.binding_id.clone(),
        binding_key: B64.encode(binding.key),
        expires_at: binding.expires_at,
      },
    ),
    forwards: Vec::new(),
    // the caller is now reachable: this channel is its standing one
    joined: Some(binding),
    ephemeral: None,
  }
}

fn handle_heartbeat(
  state: &State,
  req: &Request,
  hb: &HeartbeatRequest,
  now: UnixSeconds,
) -> Response {
  if hb.timestamp.abs_diff(now) > CLOCK_TOLERANCE {
    return Response::err(&req.id, "stale_timestamp", None);
  }
  match state.bindings.heartbeat_authenticated(
    &hb.binding_id,
    hb.seq,
    hb.generation,
    now,
    |binding| {
      verify_binding_mac(
        &binding.key,
        &hb.binding_id,
        hb.seq,
        hb.timestamp,
        hb.generation,
        &hb.mac,
      )
    },
  ) {
    Ok(binding) => Response::ok(
      &req.id,
      &HeartbeatResponse {
        expires_at: binding.expires_at,
      },
    ),
    Err(error) => Response::err(&req.id, error.code(), Some(error.to_string())),
  }
}

fn handle_exit(state: &State, req: &Request, ex: &ExitRequest, now: UnixSeconds) -> Response {
  if ex.timestamp.abs_diff(now) > CLOCK_TOLERANCE {
    return Response::err(&req.id, "stale_timestamp", None);
  }
  match state
    .bindings
    .exit_authenticated(&ex.binding_id, ex.seq, ex.generation, now, |binding| {
      q2q_punch::message::verify_exit_mac(
        &binding.key,
        &ex.binding_id,
        ex.seq,
        ex.timestamp,
        ex.generation,
        &ex.mac,
      )
    }) {
    Ok(_) => Response::ok(&req.id, &serde_json::json!({ "ok": true })),
    Err(error) => Response::err(&req.id, error.code(), Some(error.to_string())),
  }
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
  lifecycle: Mutex<()>,
  authenticated_peer: Mutex<Option<(PeerId, String)>>,
  ephemeral_ids: Mutex<Vec<String>>,
}

impl PunchChannel {
  fn new(sink: ChannelSink) -> Self {
    Self {
      sink,
      lifecycle: Mutex::new(()),
      authenticated_peer: Mutex::new(None),
      ephemeral_ids: Mutex::new(Vec::new()),
    }
  }

  async fn dispatch(&self, state: &State, req: Request, ip: std::net::IpAddr) -> Response {
    let signal = if req.method == "punch.signal" {
      serde_json::from_value::<SignalRequest>(req.params.clone()).ok()
    } else {
      None
    };
    let serial = signal
      .as_ref()
      .map(|signal| state.sinks.connection_lock(&signal.connection_id));
    let _guard = match serial {
      Some(lock) => Some(lock.lock_owned().await),
      None => None,
    };
    let outcome = match self.handle_request(state, req, ip, signal.as_ref()) {
      Ok(outcome) => outcome,
      Err(response) => return response,
    };
    if let Err(error) = crate::signalling::deliver(state, outcome.forwards).await {
      return Response::err(&outcome.response.id, error, None);
    }
    outcome.response
  }

  fn handle_request(
    &self,
    state: &State,
    req: Request,
    ip: IpAddr,
    signal: Option<&SignalRequest>,
  ) -> Result<Outcome, Response> {
    // QUIC streams may execute concurrently. Protect channel identity and route
    // registration together, and release this guard before any delivery I/O.
    let _lifecycle = self.lifecycle.lock();
    let ephemeral_peer = signal
      .filter(|sig| {
        self.ephemeral_ids.lock().contains(&sig.connection_id)
          && state
            .sinks
            .ephemeral_is_current(&sig.connection_id, &self.sink)
      })
      .and_then(|sig| state.connections.get(&sig.connection_id))
      .filter(|connection| connection.expires_at > now())
      .map(|connection| connection.a);
    let caller = Caller {
      peer: self.current_peer(state).or(ephemeral_peer),
    };
    if signal.is_some_and(|sig| !self.ephemeral_ids.lock().contains(&sig.connection_id))
      && self.ephemeral_ids.lock().len() >= 64
    {
      return Err(Response::err(&req.id, "too_many_connections", None));
    }
    let outcome = handle_guarded(state, req, &caller, ip);

    if let Some(binding) = &outcome.joined {
      if !state
        .sinks
        .put_standing(binding, self.sink.clone(), &state.bindings)
      {
        return Err(Response::err(
          &outcome.response.id,
          "binding_replaced",
          None,
        ));
      }
      let previous = self
        .authenticated_peer
        .lock()
        .replace((binding.peer_id.clone(), binding.binding_id.clone()));
      if let Some((peer, id)) = previous
        && id != binding.binding_id
      {
        if peer != binding.peer_id {
          state.sinks.drop_standing_if_current(&peer, &self.sink);
        }
        state.bindings.remove_if_current(&id);
      }
    }
    if let Some(connection_id) = &outcome.ephemeral {
      state.sinks.put_ephemeral(connection_id, self.sink.clone());
      let mut ids = self.ephemeral_ids.lock();
      if !ids.contains(connection_id) {
        ids.push(connection_id.clone());
      }
    }
    // Exit and expiry revoke this channel's standing identity immediately.
    self.current_peer(state);
    Ok(outcome)
  }

  fn current_peer(&self, state: &State) -> Option<PeerId> {
    let mut authenticated = self.authenticated_peer.lock();
    let (peer, id) = authenticated.as_ref()?;
    if state
      .bindings
      .find(id)
      .is_some_and(|binding| binding.expires_at > now())
      && state.sinks.standing_is_current(peer, id, &self.sink)
    {
      return Some(peer.clone());
    }
    state.sinks.drop_standing_if_current(peer, &self.sink);
    *authenticated = None;
    None
  }

  fn is_authenticated(&self) -> bool {
    self.authenticated_peer.lock().is_some() || !self.ephemeral_ids.lock().is_empty()
  }

  fn cleanup(&self, state: &State) {
    if let Some((peer, binding_id)) = self.authenticated_peer.lock().as_ref() {
      state.sinks.drop_standing_if_current(peer, &self.sink);
      state.bindings.remove_if_current(binding_id);
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
  let connections = state.quic_slots.clone();
  let per_ip = PerIpConnections(Arc::new(Mutex::new(HashMap::new())));
  while let Some(incoming) = endpoint.accept().await {
    let Ok(permit) = connections.clone().try_acquire_owned() else {
      incoming.refuse();
      continue;
    };
    let Some(ip_permit) = per_ip.acquire(incoming.remote_address().ip()) else {
      incoming.refuse();
      continue;
    };
    let state = state.clone();
    // Each handshake proceeds independently; one silent client cannot block accept.
    tokio::spawn(async move {
      let _permit = permit;
      let _ip_permit = ip_permit;
      match tokio::time::timeout(HANDSHAKE_TIMEOUT, incoming).await {
        Ok(Ok(connection)) => handle_connection(connection, state).await,
        Ok(Err(error)) => tracing::debug!(%error, "QUIC handshake failed"),
        Err(_) => tracing::debug!("QUIC handshake timed out"),
      }
    });
  }
}

// Erase only the socket IO type; both modes share framing, auth and limits.
trait WebSocketIo: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> WebSocketIo for T {}

pub async fn serve_websocket(
  listener: TcpListener,
  acceptor: Option<TlsAcceptor>,
  state: Arc<State>,
) -> std::io::Result<()> {
  let global_connections = state.websocket_slots.clone();
  let per_ip_connections = PerIpConnections(Arc::new(Mutex::new(HashMap::new())));
  loop {
    let (stream, remote) = listener.accept().await?;
    let Ok(global_permit) = global_connections.clone().try_acquire_owned() else {
      tracing::debug!(%remote, "WebSocket connection limit reached");
      continue;
    };
    let Some(ip_permit) = per_ip_connections.acquire(remote.ip()) else {
      tracing::debug!(%remote, "WebSocket per-IP connection limit reached");
      continue;
    };
    let st = state.clone();
    let tls = acceptor.clone();
    tokio::spawn(async move {
      let ws_config = WebSocketConfig::default()
        .max_message_size(Some(st.max_request_bytes))
        .max_frame_size(Some(st.max_request_bytes));
      let handshake = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
        let stream: Box<dyn WebSocketIo> = match tls {
          Some(tls) => Box::new(
            tls
              .accept(stream)
              .await
              .map_err(|error| error.to_string())?,
          ),
          None => Box::new(stream),
        };
        accept_async_with_config(stream, Some(ws_config))
          .await
          .map_err(|error| error.to_string())
      })
      .await;
      match handshake {
        Ok(Ok(socket)) => {
          let _global_permit = global_permit;
          let _ip_permit = ip_permit;
          handle_websocket(socket, st, remote).await;
        }
        Ok(Err(error)) => tracing::debug!(%remote, %error, "WebSocket handshake failed"),
        Err(_) => tracing::debug!(%remote, "WebSocket transport or upgrade handshake timed out"),
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
  handle_websocket_with_auth_timeout(socket, state, remote, AUTH_TIMEOUT).await;
}

async fn handle_websocket_with_auth_timeout<S>(
  socket: tokio_tungstenite::WebSocketStream<S>,
  state: Arc<State>,
  remote: SocketAddr,
  auth_timeout: std::time::Duration,
) where
  S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
  let ip = remote.ip();
  let (writer, mut reader) = socket.split();
  let (mine, mut outgoing) = ChannelSink::channel(64);
  let channel = PunchChannel::new(mine.clone());
  let (write_tx, mut write_rx) = mpsc::channel(WEBSOCKET_WRITE_QUEUE);
  let (writer_done_tx, mut writer_done_rx) = oneshot::channel();
  let mut writer_task = tokio::spawn(async move {
    let mut writer = writer;
    loop {
      tokio::select! {
        pending = outgoing.recv() => {
          let Some(pending) = pending else { break; };
          let delivered = match serde_json::to_vec(&PunchFrame::Signal(pending.signal)) {
            Ok(bytes) => writer.send(Message::Binary(bytes.into())).await
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
        message = write_rx.recv() => {
          let Some(message) = message else { break; };
          let result = match message {
            WebSocketWrite::Binary(bytes) => writer.send(Message::Binary(bytes.into())).await,
            WebSocketWrite::Pong(bytes) => writer.send(Message::Pong(bytes.into())).await,
            WebSocketWrite::Close => {
              let _ = writer.send(Message::Close(None)).await;
              break;
            }
          };
          if let Err(error) = result {
            tracing::debug!(%remote, %error, "WebSocket write failed");
            break;
          }
        }
      }
    }
    let _ = writer_done_tx.send(());
  });

  tracing::debug!(%remote, "WebSocket Punch connection accepted");
  let auth_deadline = tokio::time::sleep(auth_timeout);
  tokio::pin!(auth_deadline);
  let mut authenticated = channel.is_authenticated();

  loop {
    tokio::select! {
      _ = &mut auth_deadline, if !authenticated => {
        tracing::debug!(%remote, "WebSocket Punch authentication timed out");
        break;
      }
      _ = &mut writer_done_rx => break,
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
            let response = if let Err(error) = validate_request_version(&req) {
              Response::err(&req.id, &error.code, error.detail)
            } else {
              channel.dispatch(&state, req, ip).await
            };
            authenticated |= channel.is_authenticated();
            let bytes = match serde_json::to_vec(&PunchFrame::Response(response)) {
              Ok(bytes) => bytes,
              Err(error) => {
                tracing::error!(%error, "could not encode WebSocket Punch response");
                break;
              }
            };
            if !matches!(tokio::time::timeout(
              WRITE_TIMEOUT,
              write_tx.send(WebSocketWrite::Binary(bytes)),
            ).await, Ok(Ok(()))) {
              tracing::debug!(%remote, "WebSocket writer did not accept the response");
              break;
            }
          }
          Message::Ping(bytes) => {
            if !matches!(tokio::time::timeout(
              WRITE_TIMEOUT,
              write_tx.send(WebSocketWrite::Pong(bytes.to_vec())),
            ).await, Ok(Ok(()))) {
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
  drop(channel);
  drop(mine);
  let _ = write_tx.try_send(WebSocketWrite::Close);
  if tokio::time::timeout(std::time::Duration::from_secs(2), &mut writer_task)
    .await
    .is_err()
  {
    writer_task.abort();
  }
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
  let writer_task = tokio::spawn(async move {
    while let Some(pending) = outgoing.recv().await {
      if pending.delivered.is_closed() {
        continue;
      }
      let delivered = matches!(
        tokio::time::timeout(WRITE_TIMEOUT, async {
          let mut send = writer.open_uni().await.ok()?;
          let bytes = serde_json::to_vec(&pending.signal).ok()?;
          send.write_all(&bytes).await.ok()?;
          send.finish().ok()
        })
        .await,
        Ok(Some(()))
      );
      let _ = pending.delivered.send(delivered);
      if !delivered {
        writer.close(0u32.into(), b"signal delivery failed");
        break;
      }
    }
  });

  // Retain best-effort heartbeat support for older clients.
  let datagrams = conn.clone();
  let dg_state = state.clone();
  let datagram_task = tokio::spawn(async move {
    while let Ok(bytes) = datagrams.read_datagram().await {
      let outcome = match parse_request(&bytes) {
        Ok(req) if req.method == "punch.heartbeat" => {
          handle_guarded(&dg_state, req, &Caller::default(), ip)
        }
        Ok(req) => Outcome {
          response: Response::err(&req.id, "stream_required", None),
          forwards: Vec::new(),
          joined: None,
          ephemeral: None,
        },
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

  let mut requests = tokio::task::JoinSet::new();
  let auth_deadline = tokio::time::Instant::now() + AUTH_TIMEOUT;
  loop {
    let streams = tokio::select! {
      biased;
      _ = requests.join_next(), if !requests.is_empty() => continue,
      _ = tokio::time::sleep_until(auth_deadline), if !channel.is_authenticated() => {
        if !channel.is_authenticated() { conn.close(0u32.into(), b"authentication timed out"); break; }
        continue;
      }
      streams = conn.accept_bi() => streams,
    };
    let Ok((mut send, mut recv)) = streams else {
      break;
    };
    if requests.len() >= 32 {
      conn.close(0u32.into(), b"too many concurrent requests");
      break;
    }
    let state = state.clone();
    let channel = channel.clone();
    requests.spawn(async move {
      let Ok(Ok(bytes)) =
        tokio::time::timeout(HANDSHAKE_TIMEOUT, recv.read_to_end(state.max_request_bytes)).await
      else {
        return;
      };
      let response = match parse_request(&bytes) {
        Ok(req) => channel.dispatch(&state, req, ip).await,
        Err(error) => Response::err("unknown", &error.code, error.detail),
      };
      let Ok(bytes) = serde_json::to_vec(&response) else {
        return;
      };
      if matches!(
        tokio::time::timeout(WRITE_TIMEOUT, send.write_all(&bytes)).await,
        Ok(Ok(()))
      ) {
        let _ = send.finish();
      }
    });
  }
  // No request may register a route after its transport has been cleaned up.
  requests.abort_all();
  while requests.join_next().await.is_some() {}
  writer_task.abort();
  datagram_task.abort();
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
  use q2q_primitives::identity::{
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
      quic_slots: Arc::new(Semaphore::new(CONNECTION_LIMIT)),
      websocket_slots: Arc::new(Semaphore::new(CONNECTION_LIMIT)),
    }
  }

  type TestWebSocket = tokio_tungstenite::WebSocketStream<tokio::io::DuplexStream>;

  async fn websocket_client(
    state: Arc<State>,
    port: u16,
  ) -> (TestWebSocket, tokio::task::JoinHandle<()>) {
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let server_socket = tokio_tungstenite::WebSocketStream::from_raw_socket(
      server_io,
      tokio_tungstenite::tungstenite::protocol::Role::Server,
      None,
    )
    .await;
    let client_socket = tokio_tungstenite::WebSocketStream::from_raw_socket(
      client_io,
      tokio_tungstenite::tungstenite::protocol::Role::Client,
      None,
    )
    .await;
    let handler = tokio::spawn(async move {
      handle_websocket(
        server_socket,
        state,
        format!("127.0.0.1:{port}").parse().unwrap(),
      )
      .await;
    });
    (client_socket, handler)
  }

  async fn send_ws_request(socket: &mut TestWebSocket, request: Request) {
    socket
      .send(tokio_tungstenite::tungstenite::Message::Binary(
        serde_json::to_vec(&PunchFrame::Request(request))
          .unwrap()
          .into(),
      ))
      .await
      .unwrap();
  }

  async fn recv_ws_frame(socket: &mut TestWebSocket) -> PunchFrame {
    let message = tokio::time::timeout(std::time::Duration::from_secs(2), socket.next())
      .await
      .expect("timed out waiting for a WebSocket Punch frame")
      .unwrap()
      .unwrap();
    let tokio_tungstenite::tungstenite::Message::Binary(bytes) = message else {
      panic!("Punch WebSocket frames must be binary");
    };
    serde_json::from_slice(&bytes).unwrap()
  }

  async fn close_ws_client(socket: &mut TestWebSocket, handler: tokio::task::JoinHandle<()>) {
    let _ = socket
      .send(tokio_tungstenite::tungstenite::Message::Close(None))
      .await;
    tokio::time::timeout(std::time::Duration::from_secs(2), handler)
      .await
      .expect("WebSocket handler did not stop after close")
      .unwrap();
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
      mac: q2q_punch::message::exit_mac(&key, &joined.binding_id, 2, now, 0),
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

    // Datagrams remain supported for heartbeat clients that choose best-effort delivery.
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

    let hb_stream = Request {
      v: 1,
      id: "r-3".into(),
      method: "punch.heartbeat".into(),
      params: serde_json::to_value(&HeartbeatRequest {
        binding_id: joined.binding_id.clone(),
        seq: 2,
        timestamp: now,
        generation: 0,
        mac: binding_mac(&key, &joined.binding_id, 2, now, 0),
      })
      .unwrap(),
    };
    let resp = rpc(&conn, &hb_stream).await;
    assert_eq!(resp.id, "r-3");
    assert!(resp.error.is_none(), "{:?}", resp.error);

    assert_eq!(st.quic_slots.available_permits(), CONNECTION_LIMIT - 1);
    assert_eq!(st.bindings.live_count(q2q_punch::binding::now()), 1);
    conn.close(0u32.into(), b"done");
    client.wait_idle().await;
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
      while st.quic_slots.available_permits() != CONNECTION_LIMIT {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
      }
    })
    .await
    .unwrap();
  }

  async fn websocket_listener_join(tls_enabled: bool) {
    let identity = crate::cert::PunchIdentity::generate("punch-1").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let acceptor =
      tls_enabled.then(|| TlsAcceptor::from(Arc::new(identity.tls_server_config().unwrap())));
    let st = Arc::new(state(60));
    let server = tokio::spawn(serve_websocket(listener, acceptor, st.clone()));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
      let tcp = tokio::net::TcpStream::connect(address).await.unwrap();
      let stream: Box<dyn WebSocketIo> = if tls_enabled {
        // Fixture certificate only: production native clients pin the Track key.
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(
          rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAny))
        .with_no_client_auth();
        Box::new(
          tokio_rustls::TlsConnector::from(Arc::new(config))
            .connect("punch-1".try_into().unwrap(), tcp)
            .await
            .unwrap(),
        )
      } else {
        Box::new(tcp)
      };
      let scheme = if tls_enabled { "wss" } else { "ws" };
      let (mut socket, _) =
        tokio_tungstenite::client_async(format!("{scheme}://{address}/"), stream)
          .await
          .unwrap();
      let request = Request {
        v: 1,
        id: "listener-join".into(),
        method: "punch.join".into(),
        params: serde_json::to_value(join_request(11, "punch-1", now())).unwrap(),
      };
      socket
        .send(Message::Binary(
          serde_json::to_vec(&PunchFrame::Request(request))
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();
      let Message::Binary(bytes) = socket.next().await.unwrap().unwrap() else {
        panic!("expected binary response");
      };
      let PunchFrame::Response(response) = serde_json::from_slice(&bytes).unwrap() else {
        panic!("expected Punch response");
      };
      assert_eq!(response.id, "listener-join");
      assert!(response.error.is_none());
      assert!(response.result.is_some());
      socket.close(None).await.unwrap();
      while let Some(message) = socket.next().await {
        if matches!(message, Ok(Message::Close(_)) | Err(_)) {
          break;
        }
      }
      // The session must release the authenticated binding in either mode.
      while st.websocket_slots.available_permits() != CONNECTION_LIMIT {
        tokio::task::yield_now().await;
      }
      assert!(st.bindings.get(&peer(11)).is_none());
    })
    .await
    .unwrap();
    server.abort();
    let _ = server.await;
  }

  #[tokio::test]
  async fn plain_websocket_listener_authenticates_and_cleans_up() {
    websocket_listener_join(false).await;
  }

  #[tokio::test]
  async fn tls_websocket_listener_authenticates_and_cleans_up() {
    websocket_listener_join(true).await;
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
  async fn websocket_self_connect_does_not_deadlock_and_close_cleans_binding() {
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
        "127.0.0.1:41002".parse().unwrap(),
      )
      .await;
    });

    let now = q2q_punch::binding::now();
    let peer = peer(21);
    let join = Request {
      v: 1,
      id: "self-join".into(),
      method: "punch.join".into(),
      params: serde_json::to_value(join_request(21, "punch-1", now)).unwrap(),
    };
    client_socket
      .send(tokio_tungstenite::tungstenite::Message::Binary(
        serde_json::to_vec(&PunchFrame::Request(join))
          .unwrap()
          .into(),
      ))
      .await
      .unwrap();
    let _join_response =
      tokio::time::timeout(std::time::Duration::from_secs(2), client_socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(st.sinks.has_standing(&peer));

    let mut connect = signal(SignalType::Connect, "self-connect");
    connect.connect_token = Some(connect_token(21, "punch-1", &peer, "self-connect", now));
    client_socket
      .send(tokio_tungstenite::tungstenite::Message::Binary(
        serde_json::to_vec(&PunchFrame::Request(Request {
          v: 1,
          id: "self-connect-rpc".into(),
          method: "punch.signal".into(),
          params: serde_json::to_value(connect).unwrap(),
        }))
        .unwrap()
        .into(),
      ))
      .await
      .unwrap();

    let mut saw_signal = false;
    let mut saw_response = false;
    for _ in 0..2 {
      let message = tokio::time::timeout(std::time::Duration::from_secs(2), client_socket.next())
        .await
        .expect("self-connect should not deadlock")
        .unwrap()
        .unwrap();
      let tokio_tungstenite::tungstenite::Message::Binary(bytes) = message else {
        panic!("server frames must be binary");
      };
      match serde_json::from_slice::<PunchFrame>(&bytes).unwrap() {
        PunchFrame::Signal(signal) => {
          assert_eq!(signal.connection_id, "self-connect");
          saw_signal = true;
        }
        PunchFrame::Response(response) => {
          assert_eq!(response.id, "self-connect-rpc");
          assert!(response.error.is_none(), "{:?}", response.error);
          saw_response = true;
        }
        PunchFrame::Request(_) => panic!("server must not send request frames"),
      }
    }
    assert!(saw_signal && saw_response);

    client_socket
      .send(tokio_tungstenite::tungstenite::Message::Close(None))
      .await
      .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), handler)
      .await
      .unwrap()
      .unwrap();
    assert!(!st.sinks.has_standing(&peer));
  }

  #[tokio::test]
  async fn websocket_simultaneous_signal_forwards_do_not_deadlock() {
    let st = Arc::new(state(60));
    let (mut a, a_handler) = websocket_client(st.clone(), 41005).await;
    let (mut b, b_handler) = websocket_client(st.clone(), 41006).await;
    let now = q2q_punch::binding::now();
    let b_peer = peer(32);

    send_ws_request(
      &mut b,
      Request {
        v: 1,
        id: "b-join".into(),
        method: "punch.join".into(),
        params: serde_json::to_value(join_request(32, "punch-1", now)).unwrap(),
      },
    )
    .await;
    let PunchFrame::Response(response) = recv_ws_frame(&mut b).await else {
      panic!("join should return an RPC response");
    };
    assert!(response.error.is_none(), "{:?}", response.error);

    let connection_id = "ws-bidirectional";
    let mut connect = signal(SignalType::Connect, connection_id);
    connect.connect_token = Some(connect_token(31, "punch-1", &b_peer, connection_id, now));
    send_ws_request(
      &mut a,
      Request {
        v: 1,
        id: "a-connect".into(),
        method: "punch.signal".into(),
        params: serde_json::to_value(connect.clone()).unwrap(),
      },
    )
    .await;
    let PunchFrame::Signal(forwarded) = recv_ws_frame(&mut b).await else {
      panic!("target should receive connect before A receives its response");
    };
    assert_eq!(forwarded.kind, SignalType::Connect);
    let PunchFrame::Response(response) = recv_ws_frame(&mut a).await else {
      panic!("connect should return an RPC response");
    };
    assert!(response.error.is_none(), "{:?}", response.error);

    send_ws_request(
      &mut b,
      Request {
        v: 1,
        id: "b-accept".into(),
        method: "punch.signal".into(),
        params: serde_json::to_value(signal(SignalType::Accept, connection_id)).unwrap(),
      },
    )
    .await;
    let PunchFrame::Signal(forwarded) = recv_ws_frame(&mut a).await else {
      panic!("initiator should receive accept");
    };
    assert_eq!(forwarded.kind, SignalType::Accept);
    let PunchFrame::Response(response) = recv_ws_frame(&mut b).await else {
      panic!("accept should return an RPC response");
    };
    assert!(response.error.is_none(), "{:?}", response.error);

    let mut offer = signal(SignalType::Offer, connection_id);
    offer.connect_token = connect.connect_token.clone();
    offer.sdp = Some("v=0".into());
    send_ws_request(
      &mut a,
      Request {
        v: 1,
        id: "a-offer".into(),
        method: "punch.signal".into(),
        params: serde_json::to_value(offer).unwrap(),
      },
    )
    .await;
    let PunchFrame::Signal(forwarded) = recv_ws_frame(&mut b).await else {
      panic!("target should receive the offer");
    };
    assert_eq!(forwarded.kind, SignalType::Offer);
    let PunchFrame::Response(response) = recv_ws_frame(&mut a).await else {
      panic!("offer should return an RPC response");
    };
    assert!(response.error.is_none(), "{:?}", response.error);

    let mut candidate_a = signal(SignalType::Candidate, connection_id);
    candidate_a.candidate = Some("candidate:a".into());
    let mut candidate_b = signal(SignalType::Candidate, connection_id);
    candidate_b.candidate = Some("candidate:b".into());
    send_ws_request(
      &mut a,
      Request {
        v: 1,
        id: "a-candidate".into(),
        method: "punch.signal".into(),
        params: serde_json::to_value(candidate_a).unwrap(),
      },
    )
    .await;
    send_ws_request(
      &mut b,
      Request {
        v: 1,
        id: "b-candidate".into(),
        method: "punch.signal".into(),
        params: serde_json::to_value(candidate_b).unwrap(),
      },
    )
    .await;

    let (a_frames, b_frames) = tokio::join!(
      async { [recv_ws_frame(&mut a).await, recv_ws_frame(&mut a).await] },
      async { [recv_ws_frame(&mut b).await, recv_ws_frame(&mut b).await] },
    );
    assert!(a_frames.iter().any(|frame| matches!(frame,
      PunchFrame::Signal(signal) if signal.candidate.as_deref() == Some("candidate:b")
    )));
    assert!(a_frames.iter().any(|frame| matches!(frame,
      PunchFrame::Response(response) if response.id == "a-candidate" && response.error.is_none()
    )));
    assert!(b_frames.iter().any(|frame| matches!(frame,
      PunchFrame::Signal(signal) if signal.candidate.as_deref() == Some("candidate:a")
    )));
    assert!(b_frames.iter().any(|frame| matches!(frame,
      PunchFrame::Response(response) if response.id == "b-candidate" && response.error.is_none()
    )));

    close_ws_client(&mut a, a_handler).await;
    close_ws_client(&mut b, b_handler).await;
  }

  #[tokio::test]
  async fn websocket_rejects_unsupported_request_versions_before_dispatch() {
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
        "127.0.0.1:41003".parse().unwrap(),
      )
      .await;
    });

    let now = q2q_punch::binding::now();
    let join = join_request(22, "punch-1", now);
    for (id, version) in [("unsupported", 2), ("supported", 1)] {
      client_socket
        .send(tokio_tungstenite::tungstenite::Message::Binary(
          serde_json::to_vec(&PunchFrame::Request(Request {
            v: version,
            id: id.into(),
            method: "punch.join".into(),
            params: serde_json::to_value(&join).unwrap(),
          }))
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
        panic!("server response must be binary");
      };
      let PunchFrame::Response(response) = serde_json::from_slice(&bytes).unwrap() else {
        panic!("server should answer the request frame");
      };
      assert_eq!(response.id, id);
      if version == 2 {
        assert_eq!(response.error.unwrap().code, "unsupported_version");
        assert_eq!(st.bindings.len(), 0);
      } else {
        assert!(response.error.is_none());
        assert_eq!(st.bindings.len(), 1);
      }
    }

    client_socket
      .send(tokio_tungstenite::tungstenite::Message::Close(None))
      .await
      .unwrap();
    handler.await.unwrap();
  }

  #[tokio::test]
  async fn websocket_closes_channels_that_never_authenticate() {
    let st = Arc::new(state(60));
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let server_socket = tokio_tungstenite::WebSocketStream::from_raw_socket(
      server_io,
      tokio_tungstenite::tungstenite::protocol::Role::Server,
      None,
    )
    .await;
    let _client_socket = tokio_tungstenite::WebSocketStream::from_raw_socket(
      client_io,
      tokio_tungstenite::tungstenite::protocol::Role::Client,
      None,
    )
    .await;
    tokio::time::timeout(
      std::time::Duration::from_secs(1),
      handle_websocket_with_auth_timeout(
        server_socket,
        st.clone(),
        "127.0.0.1:41004".parse().unwrap(),
        std::time::Duration::from_millis(20),
      ),
    )
    .await
    .expect("unauthenticated WebSocket should be closed at its deadline");
    assert_eq!(st.bindings.len(), 0);
    assert!(!st.sinks.has_standing(&peer(23)));
  }

  #[test]
  fn websocket_per_ip_connection_quota_is_released_on_drop() {
    let counts = PerIpConnections(Arc::new(Mutex::new(HashMap::new())));
    let ip = "192.0.2.10".parse().unwrap();
    let mut permits = (0..CONNECTIONS_PER_IP)
      .map(|_| counts.acquire(ip).unwrap())
      .collect::<Vec<_>>();
    assert!(counts.acquire(ip).is_none());
    permits.pop();
    assert!(counts.acquire(ip).is_some());
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
    let body: q2q_primitives::identity::TicketBody = serde_json::from_str(&ticket).unwrap();
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
  #[test]
  fn offer_tickets_must_match_the_original_connection_scope() {
    let now = 1_000_000;
    for field in ["cid", "tgt", "ih"] {
      let st = state(300);
      let b = peer(2);
      let token = connect_token(1, "punch-1", &b, "original", now);
      let body: q2q_primitives::identity::TicketBody = serde_json::from_str(&token).unwrap();
      st.connections.open(&body, &b, true, now).unwrap();
      st.connections.accept("original", &b, now).unwrap();
      let mut ticket = body.to_ticket().unwrap();
      match field {
        "cid" => ticket.cid = Some("other".into()),
        "tgt" => ticket.tgt = Some(peer(3)),
        _ => ticket.ih = Some([8; 32]),
      }
      ticket.sign(&keypair_from_seed(&[1; 32]).0);
      let mut offer = signal(SignalType::Offer, "original");
      offer.sdp = Some("offer".into());
      offer.connect_token = Some(serde_json::to_string(&ticket.to_body()).unwrap());
      let response = call_with(
        &st,
        "punch.signal",
        serde_json::to_value(offer).unwrap(),
        Caller {
          peer: Some(peer(1)),
        },
      )
      .response;
      assert_eq!(response.error.unwrap().code, "unauthorized", "{field}");
      assert!(st.connections.get("original").unwrap().signal_key.is_none());
    }
  }

  #[tokio::test]
  async fn old_channel_cleanup_preserves_a_replacement_join() {
    let st = state(300);
    let now = now();
    let (sink_a, _receiver_a) = ChannelSink::channel(1);
    let channel_a = PunchChannel::new(sink_a);
    let (sink_b, _receiver_b) = ChannelSink::channel(1);
    let channel_b = PunchChannel::new(sink_b);
    for (channel, time) in [(&channel_a, now), (&channel_b, now + 1)] {
      let response = channel
        .dispatch(
          &st,
          Request {
            v: 1,
            id: "join".into(),
            method: "punch.join".into(),
            params: serde_json::to_value(join_request(2, "punch-1", time)).unwrap(),
          },
          "127.0.0.1".parse().unwrap(),
        )
        .await;
      assert!(response.error.is_none());
    }
    let current = st.bindings.get(&peer(2)).unwrap();
    assert!(channel_a.current_peer(&st).is_none());
    channel_a.cleanup(&st);
    assert_eq!(
      st.bindings.get(&peer(2)).unwrap().binding_id,
      current.binding_id
    );
    assert_eq!(channel_b.current_peer(&st), Some(peer(2)));
    channel_b.cleanup(&st);
    assert!(st.bindings.get(&peer(2)).is_none());
  }

  #[tokio::test]
  async fn expired_and_exited_bindings_revoke_standing_identity() {
    for expire in [true, false] {
      let st = state(300);
      let now = now();
      let (sink, _receiver) = ChannelSink::channel(1);
      let channel = PunchChannel::new(sink);
      let response = channel
        .dispatch(
          &st,
          Request {
            v: 1,
            id: "join".into(),
            method: "punch.join".into(),
            params: serde_json::to_value(join_request(2, "punch-1", now)).unwrap(),
          },
          "127.0.0.1".parse().unwrap(),
        )
        .await;
      assert!(response.error.is_none());
      let mut binding = st.bindings.get(&peer(2)).unwrap();
      if expire {
        binding.expires_at = now - 1;
        st.bindings.insert(binding);
        st.bindings.sweep(now);
      } else {
        let exit = ExitRequest {
          binding_id: binding.binding_id.clone(),
          seq: 1,
          timestamp: now,
          generation: 0,
          mac: q2q_punch::message::exit_mac(&binding.key, &binding.binding_id, 1, now, 0),
        };
        let response = channel
          .dispatch(
            &st,
            Request {
              v: 1,
              id: "exit".into(),
              method: "punch.exit".into(),
              params: serde_json::to_value(exit).unwrap(),
            },
            "127.0.0.1".parse().unwrap(),
          )
          .await;
        assert!(response.error.is_none());
      }
      assert!(channel.current_peer(&st).is_none());
      let mut connect = signal(SignalType::Connect, "offline");
      connect.connect_token = Some(connect_token(1, "punch-1", &peer(2), "offline", now));
      let response = dispatch(
        &st,
        &Request {
          v: 1,
          id: "connect".into(),
          method: "punch.signal".into(),
          params: serde_json::to_value(connect).unwrap(),
        },
        now,
        &Caller::default(),
      )
      .response;
      assert_eq!(response.error.unwrap().code, "peer_offline");
      assert!(!st.sinks.has_standing(&peer(2)));
    }
  }

  #[test]
  fn exit_rejects_stale_sequence_clock_generation_and_heartbeat_credentials() {
    let st = state(300);
    let now = 1_000_000;
    let response = call(
      &st,
      "punch.join",
      serde_json::to_value(join_request(2, "punch-1", now)).unwrap(),
    );
    assert!(response.error.is_none());
    let binding = st.bindings.get(&peer(2)).unwrap();
    st.bindings
      .heartbeat_authenticated(&binding.binding_id, 5, 3, now, |_| true)
      .unwrap();
    for (seq, timestamp, generation, heartbeat_mac, expected) in [
      (5, now, 3, false, "stale_sequence"),
      (6, now - 61, 3, false, "stale_timestamp"),
      (6, now, 2, false, "stale_generation"),
      (6, now, 3, true, "bad_mac"),
    ] {
      let mac = if heartbeat_mac {
        binding_mac(
          &binding.key,
          &binding.binding_id,
          seq,
          timestamp,
          generation,
        )
      } else {
        q2q_punch::message::exit_mac(
          &binding.key,
          &binding.binding_id,
          seq,
          timestamp,
          generation,
        )
      };
      let request = ExitRequest {
        binding_id: binding.binding_id.clone(),
        seq,
        timestamp,
        generation,
        mac,
      };
      assert_eq!(
        call(&st, "punch.exit", serde_json::to_value(request).unwrap())
          .error
          .unwrap()
          .code,
        expected
      );
      assert!(st.bindings.find(&binding.binding_id).is_some());
    }
  }

  #[tokio::test]
  async fn failed_signal_delivery_is_buffered_before_concurrent_resume() {
    let st = Arc::new(state(300));
    let now = now();
    let b = peer(2);
    let body: q2q_primitives::identity::TicketBody =
      serde_json::from_str(&connect_token(1, "punch-1", &b, "resume-race", now)).unwrap();
    st.connections.open(&body, &b, true, now).unwrap();
    st.connections.accept("resume-race", &b, now).unwrap();
    let connection = st
      .connections
      .offer("resume-race", &peer(1), "offer", now)
      .unwrap();
    let (old_sink, mut writes) = ChannelSink::channel(1);
    st.sinks.put_ephemeral("resume-race", old_sink);
    let delivering = st.clone();
    let delivery = tokio::spawn(async move {
      let _guard = delivering
        .sinks
        .connection_lock("resume-race")
        .lock_owned()
        .await;
      crate::signalling::deliver(
        &delivering,
        vec![crate::signalling::Forward {
          to: crate::signalling::Recipient::Initiator("resume-race".into()),
          signal: signal(SignalType::Answer, "resume-race"),
        }],
      )
      .await
    });
    let pending = tokio::time::timeout(std::time::Duration::from_secs(2), writes.recv())
      .await
      .unwrap()
      .unwrap();
    let resuming = st.clone();
    let resumed = tokio::spawn(async move {
      let (sink, _writes) = ChannelSink::channel(1);
      let channel = PunchChannel::new(sink);
      let mut resume = signal(SignalType::Offer, "resume-race");
      resume.signal_key = Some(B64.encode(connection.signal_key.unwrap()));
      channel
        .dispatch(
          &resuming,
          Request {
            v: 1,
            id: "resume".into(),
            method: "punch.signal".into(),
            params: serde_json::to_value(resume).unwrap(),
          },
          "127.0.0.1".parse().unwrap(),
        )
        .await
    });
    pending.delivered.send(false).unwrap();
    assert!(delivery.await.unwrap().is_ok());
    let response = resumed.await.unwrap();
    assert!(response.error.is_none());
    let result: SignalResponse = serde_json::from_value(response.result.unwrap()).unwrap();
    assert_eq!(result.pending.len(), 1);
    assert_eq!(
      serde_json::from_str::<SignalRequest>(&result.pending[0])
        .unwrap()
        .kind,
      SignalType::Answer
    );
    assert_eq!(
      crate::signalling::deliver(
        &st,
        vec![crate::signalling::Forward {
          to: crate::signalling::Recipient::Peer(peer(9)),
          signal: signal(SignalType::Offer, "missing")
        }]
      )
      .await,
      Err("signal_delivery_failed")
    );
  }

  #[tokio::test]
  async fn a_stalled_quic_handshake_does_not_block_other_clients() {
    let identity = crate::cert::PunchIdentity::generate("punch-1").unwrap();
    let server = endpoint(&identity, "127.0.0.1:0".parse().unwrap()).unwrap();
    let addr = server.local_addr().unwrap();
    let running = tokio::spawn(serve(server.clone(), Arc::new(state(300))));
    let relay = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let stalled_client = test_client();
    let stalled = tokio::spawn(
      stalled_client
        .connect(relay.local_addr().unwrap(), "punch-1")
        .unwrap(),
    );
    let mut packet = [0; 65536];
    let (len, _) = tokio::time::timeout(
      std::time::Duration::from_secs(2),
      relay.recv_from(&mut packet),
    )
    .await
    .unwrap()
    .unwrap();
    relay.send_to(&packet[..len], addr).await.unwrap();
    // Observe the server's handshake response but never forward it to this client.
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
      loop {
        let (len, from) = relay.recv_from(&mut packet).await.unwrap();
        if from == addr {
          break;
        }
        relay.send_to(&packet[..len], addr).await.unwrap();
      }
    })
    .await
    .unwrap();
    let client = test_client();
    let connection = tokio::time::timeout(
      std::time::Duration::from_secs(2),
      client.connect(addr, "punch-1").unwrap(),
    )
    .await
    .expect("a stalled handshake blocked another client")
    .unwrap();
    connection.close(0u32.into(), b"done");
    stalled_client.close(0u32.into(), b"done");
    server.close(0u32.into(), b"done");
    stalled.abort();
    running.abort();
  }
  #[test]
  fn a_forwarded_ticket_does_not_authenticate_the_offer_sender() {
    let st = state(300);
    let now = 1_000_000;
    let b = peer(2);
    let token = connect_token(1, "punch-1", &b, "private-offer", now);
    let body = serde_json::from_str(&token).unwrap();
    st.connections.open(&body, &b, true, now).unwrap();
    st.connections.accept("private-offer", &b, now).unwrap();
    let mut offer = signal(SignalType::Offer, "private-offer");
    offer.connect_token = Some(token);
    offer.sdp = Some("offer".into());
    for caller in [Caller::default(), Caller { peer: Some(b) }] {
      let response = call_with(
        &st,
        "punch.signal",
        serde_json::to_value(&offer).unwrap(),
        caller,
      )
      .response;
      assert_eq!(response.error.unwrap().code, "unauthorized");
      assert!(
        st.connections
          .get("private-offer")
          .unwrap()
          .signal_key
          .is_none()
      );
    }
    let response = call_with(
      &st,
      "punch.signal",
      serde_json::to_value(&offer).unwrap(),
      Caller {
        peer: Some(peer(1)),
      },
    )
    .response;
    assert!(response.error.is_none());
    assert!(
      st.connections
        .get("private-offer")
        .unwrap()
        .signal_key
        .is_some()
    );
  }
}
