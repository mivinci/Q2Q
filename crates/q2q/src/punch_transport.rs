//! QUIC Binding to the Punch assigned by Track.

use crate::control::{ControlError, decode_key, hex, next_seq};
use crate::identity::Identity;
use crate::signal_router::{IncomingSignal, SignalRouter};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use q2q_core::identity::Audience;
use q2q_punch::message::{
  ExitRequest, HeartbeatRequest, HeartbeatResponse, JoinRequest, JoinResponse, Request, Response,
  SignalRequest, SignalResponse, binding_mac, join_signing_bytes,
};
use q2q_track::wire::PunchWire;
use rustls::pki_types::CertificateDer;
use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc};
use url::Url;

const RPC_VERSION: u32 = 1;
const MAX_RESPONSE_BYTES: usize = 64 * 1024;

pub(super) struct PunchBinding {
  punch_id: String,
  endpoint: quinn::Endpoint,
  connection: quinn::Connection,
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
    let (endpoint, connection) = connect_quic(assignment).await?;

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
    let response = rpc(&connection, "join", "punch.join", &join).await?;
    let joined: JoinResponse = serde_json::from_value(response)
      .map_err(|e| ControlError::Protocol(format!("bad Punch join response: {e}")))?;
    let key = decode_key(&joined.binding_key, "Punch binding_key")?;
    let receiver = Arc::new(SignalReceiver::new(connection.clone()));
    let sending = connection.clone();
    let signals = SignalRouter::new(
      move || {
        let receiver = receiver.clone();
        async move { receiver.recv().await }
      },
      move |id| {
        let connection = sending.clone();
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
          let _ = send_signal(&connection, &reject).await;
        }
      },
    );
    Ok(Self {
      punch_id: assignment.punch_id.clone(),
      endpoint,
      signals,
      connection,
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
    send_signal(&self.connection, request).await
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
    let response = rpc(&self.connection, "exit", "punch.exit", &request).await;
    self.connection.close(0u32.into(), b"client closed");
    self.endpoint.wait_idle().await;
    response.map(|_| ())
  }
}

impl Drop for PunchBinding {
  fn drop(&mut self) {
    self.connection.close(0u32.into(), b"client dropped");
    self.endpoint.close(0u32.into(), b"client dropped");
  }
}

/// The initiator talks directly to the target peer's Punch. Its connect ticket
/// authenticates this short-lived channel; it does not create a standing Binding.
pub(crate) struct PunchEphemeral {
  endpoint: quinn::Endpoint,
  connection: quinn::Connection,
  signals: SignalReceiver,
}

impl PunchEphemeral {
  pub(crate) async fn connect(assignment: &PunchWire) -> Result<Self, ControlError> {
    let (endpoint, connection) = connect_quic(assignment).await?;
    Ok(Self {
      endpoint,
      signals: SignalReceiver::new(connection.clone()),
      connection,
    })
  }

  pub(crate) async fn signal(
    &self,
    request: &SignalRequest,
  ) -> Result<SignalResponse, ControlError> {
    send_signal(&self.connection, request).await
  }

  pub(crate) async fn recv_signal(&self) -> Result<SignalRequest, ControlError> {
    self.signals.recv().await
  }
}

impl Drop for PunchEphemeral {
  fn drop(&mut self) {
    self.connection.close(0u32.into(), b"client dropped");
    self.endpoint.close(0u32.into(), b"client dropped");
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
  let endpoint = endpoint(addr, public_key)?;
  let connection = tokio::time::timeout(
    Duration::from_secs(10),
    endpoint
      .connect(addr, &assignment.punch_id)
      .map_err(|e| ControlError::Transport(e.to_string()))?,
  )
  .await
  .map_err(|_| ControlError::Transport("Punch handshake timed out".into()))?
  .map_err(|e| ControlError::Transport(e.to_string()))?;
  Ok((endpoint, connection))
}

async fn send_signal(
  connection: &quinn::Connection,
  request: &SignalRequest,
) -> Result<SignalResponse, ControlError> {
  let mut nonce = [0u8; 16];
  getrandom::fill(&mut nonce).map_err(ControlError::Random)?;
  let value = tokio::time::timeout(
    Duration::from_secs(10),
    rpc(connection, &hex(&nonce), "punch.signal", request),
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

fn endpoint(addr: SocketAddr, public_key: [u8; 32]) -> Result<quinn::Endpoint, ControlError> {
  let provider = rustls::crypto::aws_lc_rs::default_provider();
  let crypto = rustls::ClientConfig::builder_with_provider(Arc::new(provider))
    .with_safe_default_protocol_versions()
    .map_err(|e| ControlError::Transport(e.to_string()))?
    .dangerous()
    .with_custom_certificate_verifier(Arc::new(PinnedKey(public_key)))
    .with_no_client_auth();
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
  Ok(endpoint)
}

async fn rpc<T: serde::Serialize>(
  connection: &quinn::Connection,
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
  result(response, "Punch")
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
struct PinnedKey([u8; 32]);

impl rustls::client::danger::ServerCertVerifier for PinnedKey {
  fn verify_server_cert(
    &self,
    end_entity: &CertificateDer<'_>,
    _intermediates: &[CertificateDer<'_>],
    _server_name: &rustls::pki_types::ServerName<'_>,
    _ocsp_response: &[u8],
    _now: rustls::pki_types::UnixTime,
  ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
    let (rest, cert) = x509_parser::parse_x509_certificate(end_entity.as_ref())
      .map_err(|_| rustls::Error::General("Punch certificate is malformed".into()))?;
    let key = cert.public_key();
    if !rest.is_empty()
      || key.algorithm.algorithm.to_id_string() != "1.3.101.112"
      || key.subject_public_key.unused_bits != 0
      || key.subject_public_key.data.as_ref() != self.0
    {
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
