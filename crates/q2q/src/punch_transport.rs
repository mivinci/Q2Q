//! QUIC Binding to the Punch assigned by Track.

use crate::control::{ControlError, decode_key, hex, next_seq};
use crate::identity::Identity;
use crate::signal_router::{IncomingSignal, SignalRouter};
use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use futures_util::{SinkExt, StreamExt};
use q2q_core::identity::Audience;
use q2q_punch::message::{
  ExitRequest, HeartbeatRequest, HeartbeatResponse, JoinRequest, JoinResponse, Request, Response,
  SignalRequest, SignalResponse, binding_mac, join_signing_bytes,
};
use q2q_punch::transport::PunchFrame;
use q2q_track::wire::PunchWire;
use rustls::pki_types::CertificateDer;
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::{Connector, WebSocketStream, connect_async_tls_with_config};
use url::Url;

const RPC_VERSION: u32 = 1;
const MAX_RESPONSE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PunchTransportKind {
  Quic,
  WebSocket,
}

/// Common operations required by standing and ephemeral Punch channels.
/// Transport implementations own framing and connection-specific I/O; the
/// SDK keeps tickets, MACs, routing and lifecycle above this boundary.
#[async_trait]
trait PunchTransport: Send + Sync {
  fn kind(&self) -> PunchTransportKind;

  async fn rpc(&self, request: Request) -> Result<Response, ControlError>;

  async fn heartbeat(&self, request: Request) -> Result<Response, ControlError>;

  async fn recv_signal(&self) -> Result<SignalRequest, ControlError>;

  async fn close(&self);
}

struct QuicPunchTransport {
  endpoint: quinn::Endpoint,
  connection: quinn::Connection,
  signals: SignalReceiver,
}

impl QuicPunchTransport {
  async fn connect(assignment: &PunchWire) -> Result<Self, ControlError> {
    let (endpoint, connection) = connect_quic(assignment).await?;
    Ok(Self {
      endpoint,
      signals: SignalReceiver::new(connection.clone()),
      connection,
    })
  }
}

#[async_trait]
impl PunchTransport for QuicPunchTransport {
  fn kind(&self) -> PunchTransportKind {
    PunchTransportKind::Quic
  }

  async fn rpc(&self, request: Request) -> Result<Response, ControlError> {
    tokio::time::timeout(
      Duration::from_secs(10),
      rpc_request(&self.connection, request),
    )
    .await
    .map_err(|_| ControlError::Transport("Punch QUIC request timed out".into()))?
  }

  async fn heartbeat(&self, request: Request) -> Result<Response, ControlError> {
    let id = request.id.clone();
    self
      .connection
      .send_datagram(
        serde_json::to_vec(&request)
          .map_err(|e| ControlError::Protocol(e.to_string()))?
          .into(),
      )
      .map_err(|e| ControlError::Transport(e.to_string()))?;
    let bytes = tokio::time::timeout(Duration::from_secs(5), self.connection.read_datagram())
      .await
      .map_err(|_| ControlError::Transport("Punch heartbeat timed out".into()))?
      .map_err(|e| ControlError::Transport(e.to_string()))?;
    let response: Response = serde_json::from_slice(&bytes)
      .map_err(|e| ControlError::Protocol(format!("bad Punch heartbeat response: {e}")))?;
    if response.id != id {
      return Err(ControlError::Protocol(
        "Punch response id does not match request".into(),
      ));
    }
    Ok(response)
  }

  async fn recv_signal(&self) -> Result<SignalRequest, ControlError> {
    self.signals.recv().await
  }

  async fn close(&self) {
    self.connection.close(0u32.into(), b"client closed");
    self.endpoint.wait_idle().await;
  }
}

impl Drop for QuicPunchTransport {
  fn drop(&mut self) {
    self.connection.close(0u32.into(), b"client dropped");
    self.endpoint.close(0u32.into(), b"client dropped");
  }
}

enum WsCommand {
  Frame(PunchFrame),
  Pong(Vec<u8>),
  Close(oneshot::Sender<()>),
}

struct WebSocketPunchTransport {
  outgoing: mpsc::Sender<WsCommand>,
  pending: Arc<Mutex<HashMap<String, oneshot::Sender<Response>>>>,
  signals: Mutex<mpsc::Receiver<Result<SignalRequest, ControlError>>>,
  reader: tokio::task::AbortHandle,
  writer: tokio::task::AbortHandle,
}

impl WebSocketPunchTransport {
  async fn connect(assignment: &PunchWire) -> Result<Self, ControlError> {
    let address = assignment
      .websocket_url
      .as_deref()
      .ok_or_else(|| ControlError::Protocol("Punch has no WebSocket URL".into()))?;
    let url = Url::parse(address)
      .map_err(|e| ControlError::Protocol(format!("invalid Punch WebSocket URL: {e}")))?;
    if url.scheme() != "wss" {
      return Err(ControlError::Protocol(
        "Punch WebSocket URL must use wss://".into(),
      ));
    }
    let public_key = decode_key(&assignment.public_key, "Punch public_key")?;
    let pin_rejected = Arc::new(AtomicBool::new(false));
    let tls = client_tls_config(public_key, pin_rejected.clone())?;
    let ws_config = WebSocketConfig::default()
      .max_message_size(Some(MAX_RESPONSE_BYTES))
      .max_frame_size(Some(MAX_RESPONSE_BYTES));
    let connected = tokio::time::timeout(
      Duration::from_secs(10),
      connect_async_tls_with_config(
        address,
        Some(ws_config),
        false,
        Some(Connector::Rustls(Arc::new(tls))),
      ),
    )
    .await
    .map_err(|_| ControlError::Transport("Punch WebSocket handshake timed out".into()))?;
    let (socket, _) = connected.map_err(|error| {
      if pin_rejected.load(Ordering::Acquire) {
        ControlError::Authentication("Punch public key does not match Track".into())
      } else {
        ControlError::Transport(error.to_string())
      }
    })?;
    Ok(Self::from_socket(socket))
  }

  fn from_socket<S>(socket: WebSocketStream<S>) -> Self
  where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
  {
    let (mut writer, mut reader) = socket.split();
    let (outgoing, mut commands) = mpsc::channel(64);
    let pending = Arc::new(Mutex::new(
      HashMap::<String, oneshot::Sender<Response>>::new(),
    ));
    let writer_pending = pending.clone();
    let writer_task = tokio::spawn(async move {
      while let Some(command) = commands.recv().await {
        let result = match command {
          WsCommand::Frame(frame) => match serde_json::to_vec(&frame) {
            Ok(bytes) => writer.send(Message::Binary(bytes.into())).await,
            Err(error) => {
              tracing::debug!(%error, "could not encode Punch WebSocket frame");
              break;
            }
          },
          WsCommand::Pong(bytes) => writer.send(Message::Pong(bytes.into())).await,
          WsCommand::Close(ack) => {
            let _ = writer.send(Message::Close(None)).await;
            let _ = ack.send(());
            break;
          }
        };
        if let Err(error) = result {
          tracing::debug!(%error, "Punch WebSocket write failed");
          break;
        }
      }
      writer_pending.lock().await.clear();
    });

    let reader_pending = pending.clone();
    let reader_outgoing = outgoing.clone();
    let (signal_tx, signals) = mpsc::channel(64);
    let reader_task = tokio::spawn(async move {
      let mut failure = None;
      while let Some(incoming) = reader.next().await {
        let message = match incoming {
          Ok(message) => message,
          Err(error) => {
            failure = Some(error.to_string());
            break;
          }
        };
        match message {
          Message::Binary(bytes) => {
            let frame = match serde_json::from_slice::<PunchFrame>(&bytes) {
              Ok(frame) => frame,
              Err(error) => {
                failure = Some(format!("invalid Punch WebSocket frame: {error}"));
                break;
              }
            };
            match frame {
              PunchFrame::Response(response) => {
                if let Some(waiter) = reader_pending.lock().await.remove(&response.id) {
                  let _ = waiter.send(response);
                }
              }
              PunchFrame::Signal(signal) => {
                if signal_tx.send(Ok(signal)).await.is_err() {
                  break;
                }
              }
              PunchFrame::Request(_) => {
                failure = Some("server sent a Punch request frame".into());
                break;
              }
            }
          }
          Message::Ping(bytes) => {
            if reader_outgoing
              .send(WsCommand::Pong(bytes.to_vec()))
              .await
              .is_err()
            {
              break;
            }
          }
          Message::Close(_) => break,
          Message::Pong(_) | Message::Frame(_) => {}
          Message::Text(_) => {
            failure = Some("server sent a text WebSocket frame".into());
            break;
          }
        }
      }
      reader_pending.lock().await.clear();
      if let Some(error) = failure {
        let _ = signal_tx.send(Err(ControlError::Transport(error))).await;
      }
    });

    Self {
      outgoing,
      pending,
      signals: Mutex::new(signals),
      reader: reader_task.abort_handle(),
      writer: writer_task.abort_handle(),
    }
  }
}

#[async_trait]
impl PunchTransport for WebSocketPunchTransport {
  fn kind(&self) -> PunchTransportKind {
    PunchTransportKind::WebSocket
  }

  async fn rpc(&self, request: Request) -> Result<Response, ControlError> {
    let id = request.id.clone();
    let (waiter, response) = oneshot::channel();
    self.pending.lock().await.insert(id.clone(), waiter);
    if self
      .outgoing
      .send(WsCommand::Frame(PunchFrame::Request(request)))
      .await
      .is_err()
    {
      self.pending.lock().await.remove(&id);
      return Err(ControlError::Transport(
        "Punch WebSocket writer is closed".into(),
      ));
    }
    match tokio::time::timeout(Duration::from_secs(10), response).await {
      Ok(Ok(response)) => Ok(response),
      Ok(Err(_)) => Err(ControlError::Transport(
        "Punch WebSocket reader is closed".into(),
      )),
      Err(_) => {
        self.pending.lock().await.remove(&id);
        Err(ControlError::Transport(
          "Punch WebSocket request timed out".into(),
        ))
      }
    }
  }

  async fn heartbeat(&self, request: Request) -> Result<Response, ControlError> {
    self.rpc(request).await
  }

  async fn recv_signal(&self) -> Result<SignalRequest, ControlError> {
    self
      .signals
      .lock()
      .await
      .recv()
      .await
      .ok_or_else(|| ControlError::Transport("Punch WebSocket signal channel closed".into()))?
  }

  async fn close(&self) {
    let (ack, closed) = oneshot::channel();
    if self.outgoing.send(WsCommand::Close(ack)).await.is_ok() {
      let _ = tokio::time::timeout(Duration::from_secs(2), closed).await;
    }
    self.reader.abort();
    self.writer.abort();
  }
}

impl Drop for WebSocketPunchTransport {
  fn drop(&mut self) {
    self.reader.abort();
    self.writer.abort();
  }
}

async fn connect_transport(
  assignment: &PunchWire,
) -> Result<Arc<dyn PunchTransport>, ControlError> {
  connect_transport_with(
    assignment.websocket_url.is_some(),
    || async {
      QuicPunchTransport::connect(assignment)
        .await
        .map(|transport| Arc::new(transport) as Arc<dyn PunchTransport>)
    },
    || async {
      WebSocketPunchTransport::connect(assignment)
        .await
        .map(|transport| Arc::new(transport) as Arc<dyn PunchTransport>)
    },
  )
  .await
}

async fn connect_transport_with<Q, QFut, W, WFut>(
  has_websocket: bool,
  connect_quic: Q,
  connect_websocket: W,
) -> Result<Arc<dyn PunchTransport>, ControlError>
where
  Q: FnOnce() -> QFut,
  QFut: Future<Output = Result<Arc<dyn PunchTransport>, ControlError>>,
  W: FnOnce() -> WFut,
  WFut: Future<Output = Result<Arc<dyn PunchTransport>, ControlError>>,
{
  match connect_quic().await {
    Ok(transport) => Ok(transport),
    Err(error) if has_websocket && matches!(&error, ControlError::Transport(_)) => {
      tracing::info!(%error, "QUIC unavailable; trying Punch WebSocket fallback");
      connect_websocket().await
    }
    Err(error) => Err(error),
  }
}

async fn connect_and_join(
  identity: &Identity,
  assignment: &PunchWire,
) -> Result<(Arc<dyn PunchTransport>, JoinResponse), ControlError> {
  let transport = connect_transport(assignment).await?;
  match join_transport(identity, assignment, transport.as_ref()).await {
    Ok(joined) => Ok((transport, joined)),
    Err(error)
      if transport.kind() == PunchTransportKind::Quic
        && assignment.websocket_url.is_some()
        && matches!(&error, ControlError::Transport(_)) =>
    {
      tracing::info!(punch = %assignment.punch_id, %error, "QUIC join failed; retrying Punch join over WebSocket");
      transport.close().await;
      let fallback = Arc::new(WebSocketPunchTransport::connect(assignment).await?);
      let joined = join_transport(identity, assignment, fallback.as_ref()).await?;
      Ok((fallback, joined))
    }
    Err(error) => Err(error),
  }
}

async fn join_transport(
  identity: &Identity,
  assignment: &PunchWire,
  transport: &dyn PunchTransport,
) -> Result<JoinResponse, ControlError> {
  // A join ticket is single-use. Create a fresh nonce when retrying another
  // transport, because the first request may have reached Punch even if its
  // response was lost.
  let now = q2q_punch::binding::now();
  let mut nonce = [0u8; 16];
  getrandom::fill(&mut nonce).map_err(ControlError::Random)?;
  let token = identity.ticket(
    Audience::Punch(assignment.punch_id.clone()),
    now,
    Some(hex(&nonce)),
  );
  let token = serde_json::to_string(&token).map_err(|e| ControlError::Protocol(e.to_string()))?;
  let join = JoinRequest {
    join_token: token.clone(),
    public_key: B64.encode(identity.public_key()),
    signature: B64.encode(identity.sign(&join_signing_bytes(&token, now))),
    timestamp: now,
  };
  let response = rpc_value(transport, "join", "punch.join", &join).await?;
  serde_json::from_value(response)
    .map_err(|e| ControlError::Protocol(format!("bad Punch join response: {e}")))
}

pub(super) struct PunchBinding {
  punch_id: String,
  transport: Arc<dyn PunchTransport>,
  signals: SignalRouter,
  binding_id: String,
  key: [u8; 32],
  seq: Mutex<u64>,
}

impl PunchBinding {
  pub(super) async fn connect(
    identity: &Identity,
    assignment: &PunchWire,
  ) -> Result<Self, ControlError> {
    let (transport, joined) = connect_and_join(identity, assignment).await?;
    let key = decode_key(&joined.binding_key, "Punch binding_key")?;
    let receiver = transport.clone();
    let sending = transport.clone();
    tracing::debug!(transport = ?transport.kind(), punch = %assignment.punch_id, "Punch transport connected");
    let signals = SignalRouter::new(
      move || {
        let receiver = receiver.clone();
        async move { receiver.recv_signal().await }
      },
      move |id| {
        let transport = sending.clone();
        async move {
          let reject = SignalRequest {
            kind: q2q_punch::message::SignalType::Reject,
            connection_id: id,
            connect_token: None,
            sdp: None,
            candidate: None,
            reason: Some(q2q_punch::message::RejectReason::Busy),
            accept_token: None,
            signal_key: None,
          };
          let _ = send_signal(transport.as_ref(), &reject).await;
        }
      },
    );
    Ok(Self {
      punch_id: assignment.punch_id.clone(),
      transport,
      signals,
      binding_id: joined.binding_id,
      key,
      seq: Mutex::new(0),
    })
  }

  pub(super) fn punch_id(&self) -> &str {
    &self.punch_id
  }

  /// Send one signalling message over the authenticated standing channel.
  pub(crate) async fn signal(
    &self,
    request: &SignalRequest,
  ) -> Result<SignalResponse, ControlError> {
    send_signal(self.transport.as_ref(), request).await
  }

  pub(crate) async fn set_accepting(&self, enabled: bool) {
    self.signals.set_accepting(enabled).await;
  }

  pub(crate) async fn incoming(&self) -> Result<IncomingSignal, ControlError> {
    self.signals.next().await
  }

  pub(super) async fn heartbeat(&self) -> Result<(), ControlError> {
    let mut last_seq = self.seq.lock().await;
    let seq = next_seq(&mut last_seq)?;
    let now = q2q_punch::binding::now();
    let request = HeartbeatRequest {
      binding_id: self.binding_id.clone(),
      seq,
      timestamp: now,
      generation: 0,
      mac: binding_mac(&self.key, &self.binding_id, seq, now, 0),
    };
    let id = format!("heartbeat-{seq}");
    let request = Request {
      v: RPC_VERSION,
      id: id.clone(),
      method: "punch.heartbeat".into(),
      params: serde_json::to_value(request).map_err(|e| ControlError::Protocol(e.to_string()))?,
    };
    let response = self.transport.heartbeat(request).await?;
    let value = result(response, "Punch")?;
    let _: HeartbeatResponse = serde_json::from_value(value)
      .map_err(|e| ControlError::Protocol(format!("bad Punch heartbeat result: {e}")))?;
    Ok(())
  }

  pub(super) async fn close(&self) -> Result<(), ControlError> {
    let mut last_seq = self.seq.lock().await;
    let seq = next_seq(&mut last_seq)?;
    let now = q2q_punch::binding::now();
    let request = ExitRequest {
      binding_id: self.binding_id.clone(),
      seq,
      timestamp: now,
      generation: 0,
      mac: binding_mac(&self.key, &self.binding_id, seq, now, 0),
    };
    let response = rpc_value(self.transport.as_ref(), "exit", "punch.exit", &request).await;
    self.transport.close().await;
    response.map(|_| ())
  }
}

/// The initiator talks directly to the target peer's Punch. Its connect ticket
/// authenticates this short-lived channel; it does not create a standing Binding.
pub(crate) struct PunchEphemeral {
  transport: Arc<dyn PunchTransport>,
}

impl PunchEphemeral {
  pub(crate) async fn connect(assignment: &PunchWire) -> Result<Self, ControlError> {
    let transport = connect_transport(assignment).await?;
    Ok(Self { transport })
  }

  pub(crate) async fn signal(
    &self,
    request: &SignalRequest,
  ) -> Result<SignalResponse, ControlError> {
    send_signal(self.transport.as_ref(), request).await
  }

  pub(crate) async fn recv_signal(&self) -> Result<SignalRequest, ControlError> {
    self.transport.recv_signal().await
  }
}

/// Complete each QUIC push outside callers' select loops. Cancelling a receive
/// must not drop a partly read unidirectional stream and lose its ICE message.
struct SignalReceiver {
  messages: Mutex<mpsc::Receiver<Result<SignalRequest, ControlError>>>,
  reader: tokio::task::JoinHandle<()>,
}

impl SignalReceiver {
  fn new(connection: quinn::Connection) -> Self {
    let (sender, messages) = mpsc::channel(64);
    let reader = tokio::spawn(async move {
      loop {
        let result = read_signal(&connection).await;
        let failed = result.is_err();
        if sender.send(result).await.is_err() {
          break;
        }
        if failed {
          connection.close(0u32.into(), b"invalid signal stream");
          break;
        }
      }
    });
    Self {
      messages: Mutex::new(messages),
      reader,
    }
  }

  async fn recv(&self) -> Result<SignalRequest, ControlError> {
    self
      .messages
      .lock()
      .await
      .recv()
      .await
      .ok_or_else(|| ControlError::Transport("Punch signal channel closed".into()))?
  }
}

impl Drop for SignalReceiver {
  fn drop(&mut self) {
    self.reader.abort();
  }
}

async fn connect_quic(
  assignment: &PunchWire,
) -> Result<(quinn::Endpoint, quinn::Connection), ControlError> {
  let public_key = decode_key(&assignment.public_key, "Punch public_key")?;
  let url = Url::parse(&assignment.url)
    .map_err(|e| ControlError::Protocol(format!("invalid Punch URL: {e}")))?;
  if url.scheme() != "quic" {
    return Err(ControlError::Protocol("Punch URL must use quic://".into()));
  }
  let host = url
    .host_str()
    .ok_or_else(|| ControlError::Protocol("Punch URL has no host".into()))?;
  let port = url
    .port()
    .ok_or_else(|| ControlError::Protocol("Punch URL has no port".into()))?;
  let addr = tokio::net::lookup_host((host, port))
    .await
    .map_err(|e| ControlError::Transport(e.to_string()))?
    .next()
    .ok_or_else(|| ControlError::Transport("Punch host resolved to no addresses".into()))?;
  let (endpoint, pin_rejected) = endpoint(addr, public_key)?;
  let connecting = endpoint
    .connect(addr, &assignment.punch_id)
    .map_err(|e| ControlError::Transport(e.to_string()))?;
  let connection = tokio::time::timeout(Duration::from_secs(10), connecting)
    .await
    .map_err(|_| ControlError::Transport("Punch handshake timed out".into()))?
    .map_err(|e| {
      if pin_rejected.load(Ordering::Acquire) {
        ControlError::Authentication("Punch public key does not match Track".into())
      } else {
        ControlError::Transport(e.to_string())
      }
    })?;
  Ok((endpoint, connection))
}

async fn send_signal(
  transport: &dyn PunchTransport,
  request: &SignalRequest,
) -> Result<SignalResponse, ControlError> {
  let mut nonce = [0u8; 16];
  getrandom::fill(&mut nonce).map_err(ControlError::Random)?;
  let value = tokio::time::timeout(
    Duration::from_secs(10),
    rpc_value(transport, &hex(&nonce), "punch.signal", request),
  )
  .await
  .map_err(|_| ControlError::Transport("Punch signal timed out".into()))??;
  serde_json::from_value(value)
    .map_err(|e| ControlError::Protocol(format!("bad Punch signal response: {e}")))
}

async fn read_signal(connection: &quinn::Connection) -> Result<SignalRequest, ControlError> {
  let mut stream = connection
    .accept_uni()
    .await
    .map_err(|e| ControlError::Transport(e.to_string()))?;
  let bytes = tokio::time::timeout(
    Duration::from_secs(10),
    stream.read_to_end(MAX_RESPONSE_BYTES),
  )
  .await
  .map_err(|_| ControlError::Transport("Punch signal stream timed out".into()))?
  .map_err(|e| ControlError::Transport(e.to_string()))?;
  serde_json::from_slice(&bytes)
    .map_err(|e| ControlError::Protocol(format!("bad forwarded Punch signal: {e}")))
}

fn client_tls_config(
  public_key: [u8; 32],
  pin_rejected: Arc<AtomicBool>,
) -> Result<rustls::ClientConfig, ControlError> {
  let provider = rustls::crypto::aws_lc_rs::default_provider();
  let config = rustls::ClientConfig::builder_with_provider(Arc::new(provider))
    .with_safe_default_protocol_versions()
    .map_err(|e| ControlError::Transport(e.to_string()))?
    .dangerous()
    .with_custom_certificate_verifier(Arc::new(PinnedKey {
      public_key,
      rejected: pin_rejected,
    }))
    .with_no_client_auth();
  Ok(config)
}

fn endpoint(
  addr: SocketAddr,
  public_key: [u8; 32],
) -> Result<(quinn::Endpoint, Arc<AtomicBool>), ControlError> {
  let pin_rejected = Arc::new(AtomicBool::new(false));
  let crypto = client_tls_config(public_key, pin_rejected.clone())?;
  let config = quinn::crypto::rustls::QuicClientConfig::try_from(crypto)
    .map_err(|e| ControlError::Transport(e.to_string()))?;
  let bind: SocketAddr = if addr.is_ipv4() {
    "0.0.0.0:0".parse().unwrap()
  } else {
    "[::]:0".parse().unwrap()
  };
  let mut endpoint =
    quinn::Endpoint::client(bind).map_err(|e| ControlError::Transport(e.to_string()))?;
  // Track's application heartbeat may be 30s, equal to QUIC's default idle limit.
  // Transport keepalives prevent that timing race without extending binding authorization.
  let mut transport = quinn::TransportConfig::default();
  transport.keep_alive_interval(Some(Duration::from_secs(5)));
  let mut client = quinn::ClientConfig::new(Arc::new(config));
  client.transport_config(Arc::new(transport));
  endpoint.set_default_client_config(client);
  Ok((endpoint, pin_rejected))
}

async fn rpc_value<T: serde::Serialize>(
  transport: &dyn PunchTransport,
  id: &str,
  method: &str,
  params: &T,
) -> Result<serde_json::Value, ControlError> {
  let request = Request {
    v: RPC_VERSION,
    id: id.into(),
    method: method.into(),
    params: serde_json::to_value(params).map_err(|e| ControlError::Protocol(e.to_string()))?,
  };
  let response = transport.rpc(request).await?;
  result(response, "Punch")
}

async fn rpc_request(
  connection: &quinn::Connection,
  request: Request,
) -> Result<Response, ControlError> {
  let id = request.id.as_str();
  let (mut send, mut recv) = connection
    .open_bi()
    .await
    .map_err(|e| ControlError::Transport(e.to_string()))?;
  send
    .write_all(&serde_json::to_vec(&request).map_err(|e| ControlError::Protocol(e.to_string()))?)
    .await
    .map_err(|e| ControlError::Transport(e.to_string()))?;
  send
    .finish()
    .map_err(|e| ControlError::Transport(e.to_string()))?;
  let bytes = recv
    .read_to_end(MAX_RESPONSE_BYTES)
    .await
    .map_err(|e| ControlError::Transport(e.to_string()))?;
  let response: Response = serde_json::from_slice(&bytes)
    .map_err(|e| ControlError::Protocol(format!("bad Punch response: {e}")))?;
  if response.id != id {
    return Err(ControlError::Protocol(
      "Punch response id does not match request".into(),
    ));
  }
  Ok(response)
}

fn result(response: Response, service: &'static str) -> Result<serde_json::Value, ControlError> {
  if let Some(error) = response.error {
    return Err(ControlError::Remote {
      service,
      status: 0,
      code: Some(error.code),
      detail: error.detail,
    });
  }
  response
    .result
    .ok_or_else(|| ControlError::Protocol(format!("{service} response has no result")))
}

#[derive(Debug)]
struct PinnedKey {
  public_key: [u8; 32],
  rejected: Arc<AtomicBool>,
}

impl rustls::client::danger::ServerCertVerifier for PinnedKey {
  fn verify_server_cert(
    &self,
    end_entity: &CertificateDer<'_>,
    _intermediates: &[CertificateDer<'_>],
    _server_name: &rustls::pki_types::ServerName<'_>,
    _ocsp_response: &[u8],
    _now: rustls::pki_types::UnixTime,
  ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
    let (rest, cert) = x509_parser::parse_x509_certificate(end_entity.as_ref()).map_err(|_| {
      self.rejected.store(true, Ordering::Release);
      rustls::Error::General("Punch certificate is malformed".into())
    })?;
    let key = cert.public_key();
    if !rest.is_empty()
      || key.algorithm.algorithm.to_id_string() != "1.3.101.112"
      || key.subject_public_key.unused_bits != 0
      || key.subject_public_key.data.as_ref() != self.public_key
    {
      self.rejected.store(true, Ordering::Release);
      return Err(rustls::Error::General(
        "Punch public key does not match Track".into(),
      ));
    }
    Ok(rustls::client::danger::ServerCertVerified::assertion())
  }

  fn verify_tls12_signature(
    &self,
    message: &[u8],
    cert: &CertificateDer<'_>,
    dss: &rustls::DigitallySignedStruct,
  ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
    let algorithms =
      rustls::crypto::aws_lc_rs::default_provider().signature_verification_algorithms;
    rustls::crypto::verify_tls12_signature(message, cert, dss, &algorithms)
  }

  fn verify_tls13_signature(
    &self,
    message: &[u8],
    cert: &CertificateDer<'_>,
    dss: &rustls::DigitallySignedStruct,
  ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
    let algorithms =
      rustls::crypto::aws_lc_rs::default_provider().signature_verification_algorithms;
    rustls::crypto::verify_tls13_signature(message, cert, dss, &algorithms)
  }

  fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
    rustls::crypto::aws_lc_rs::default_provider()
      .signature_verification_algorithms
      .supported_schemes()
  }
}

impl fmt::Display for PinnedKey {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str("Punch public key from Track")
  }
}

#[cfg(test)]
#[path = "../tests/punch_transport.rs"]
mod tests;
