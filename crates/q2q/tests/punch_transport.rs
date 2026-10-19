use super::*;
use std::sync::atomic::AtomicUsize;

fn punch_server(idle: Option<Duration>) -> (quinn::Endpoint, PunchWire) {
  let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap();
  let cert = rcgen::CertificateParams::new(Vec::<String>::new())
    .unwrap()
    .self_signed(&key)
    .unwrap();
  let crypto = rustls::ServerConfig::builder_with_provider(Arc::new(
    rustls::crypto::aws_lc_rs::default_provider(),
  ))
  .with_safe_default_protocol_versions()
  .unwrap()
  .with_no_client_auth()
  .with_single_cert(
    vec![cert.der().clone()],
    rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
  )
  .unwrap();
  let mut config = quinn::ServerConfig::with_crypto(Arc::new(
    quinn::crypto::rustls::QuicServerConfig::try_from(crypto).unwrap(),
  ));
  if let Some(idle) = idle {
    let mut transport = quinn::TransportConfig::default();
    transport.max_idle_timeout(Some(idle.try_into().unwrap()));
    config.transport_config(Arc::new(transport));
  }
  let server = quinn::Endpoint::server(config, "127.0.0.1:0".parse().unwrap()).unwrap();
  let assignment = PunchWire {
    punch_id: "test-punch".into(),
    url: format!("quic://{}", server.local_addr().unwrap()),
    websocket_url: None,
    public_key: B64.encode(key.public_key_raw()),
  };
  (server, assignment)
}

#[tokio::test]
async fn idle_punch_connection_survives_without_application_heartbeats() {
  let (server, assignment) = punch_server(Some(Duration::from_secs(7)));
  let (accepted, connected) = tokio::join!(
    async { server.accept().await.unwrap().await.unwrap() },
    connect_quic(&assignment)
  );
  let (_endpoint, connection) = connected.unwrap();
  // Longer than the negotiated idle timeout; no RPC or heartbeat is sent by this test.
  tokio::time::sleep(Duration::from_secs(9)).await;
  assert!(connection.close_reason().is_none());
  assert!(accepted.close_reason().is_none());
  connection.close(0u32.into(), b"test complete");
}

#[tokio::test]
async fn cancelling_a_signal_wait_preserves_a_partly_received_quic_push() {
  let (server, assignment) = punch_server(None);
  let expected = SignalRequest {
    kind: q2q_punch::message::SignalType::Candidate,
    connection_id: "cancel-safe".into(),
    connect_token: None,
    sdp: None,
    candidate: Some("candidate-fragment".into()),
    reason: None,
    accept_token: None,
    signal_key: None,
  };
  let bytes = serde_json::to_vec(&expected).unwrap();
  let (prefix_written, prefix_ready) = tokio::sync::oneshot::channel();
  let (resume, resumed) = tokio::sync::oneshot::channel();
  let incoming = server.accept();
  let sending = async {
    let connection = incoming.await.unwrap().await.unwrap();
    let mut stream = connection.open_uni().await.unwrap();
    stream.write_all(&bytes[..bytes.len() / 2]).await.unwrap();
    prefix_written.send(()).unwrap();
    resumed.await.unwrap();
    stream.write_all(&bytes[bytes.len() / 2..]).await.unwrap();
    stream.finish().unwrap();
    connection
  };
  let receiving = async {
    let client = PunchEphemeral::connect(&assignment).await.unwrap();
    prefix_ready.await.unwrap();
    assert!(
      tokio::time::timeout(Duration::from_millis(30), client.recv_signal())
        .await
        .is_err()
    );
    resume.send(()).unwrap();
    let received = tokio::time::timeout(Duration::from_secs(2), client.recv_signal())
      .await
      .unwrap()
      .unwrap();
    assert_eq!(received.connection_id, expected.connection_id);
    assert_eq!(received.candidate, expected.candidate);
    client
  };
  let (_connection, _client) = tokio::join!(sending, receiving);
}

struct FakePunchTransport(PunchTransportKind);

#[async_trait::async_trait]
impl PunchTransport for FakePunchTransport {
  fn kind(&self) -> PunchTransportKind {
    self.0
  }

  async fn rpc(&self, _request: Request) -> Result<Response, ControlError> {
    unreachable!("the transport-selection tests do not issue RPCs")
  }

  async fn heartbeat(&self, _request: Request) -> Result<Response, ControlError> {
    unreachable!("the transport-selection tests do not issue heartbeats")
  }

  async fn recv_signal(&self) -> Result<SignalRequest, ControlError> {
    unreachable!("the transport-selection tests do not receive signals")
  }

  async fn close(&self) {}
}

fn fake_transport(kind: PunchTransportKind) -> Arc<dyn PunchTransport> {
  Arc::new(FakePunchTransport(kind))
}

#[tokio::test]
async fn transport_factory_prefers_quic_and_does_not_start_websocket() {
  let websocket_attempts = Arc::new(AtomicUsize::new(0));
  let attempts = websocket_attempts.clone();
  let transport = connect_transport_with(
    true,
    || async { Ok(fake_transport(PunchTransportKind::Quic)) },
    || async move {
      attempts.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
      Ok(fake_transport(PunchTransportKind::WebSocket))
    },
  )
  .await
  .unwrap();

  assert_eq!(transport.kind(), PunchTransportKind::Quic);
  assert_eq!(
    websocket_attempts.load(std::sync::atomic::Ordering::Relaxed),
    0
  );
}

#[tokio::test]
async fn transport_factory_falls_back_after_a_quic_transport_error() {
  let websocket_attempts = Arc::new(AtomicUsize::new(0));
  let attempts = websocket_attempts.clone();
  let transport = connect_transport_with(
    true,
    || async { Err(ControlError::Transport("UDP unavailable".into())) },
    || async move {
      attempts.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
      Ok(fake_transport(PunchTransportKind::WebSocket))
    },
  )
  .await
  .unwrap();

  assert_eq!(transport.kind(), PunchTransportKind::WebSocket);
  assert_eq!(
    websocket_attempts.load(std::sync::atomic::Ordering::Relaxed),
    1
  );
}

#[tokio::test]
async fn transport_factory_does_not_hide_auth_or_protocol_errors_with_fallback() {
  let errors = [
    ControlError::Authentication("wrong pinned key".into()),
    ControlError::Protocol("malformed Punch response".into()),
    ControlError::Remote {
      service: "Punch",
      status: 0,
      code: Some("unauthorized".into()),
      detail: None,
    },
  ];

  for error in errors {
    let websocket_attempts = Arc::new(AtomicUsize::new(0));
    let attempts = websocket_attempts.clone();
    let result = connect_transport_with(
      true,
      || async move { Err(error) },
      || async move {
        attempts.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(fake_transport(PunchTransportKind::WebSocket))
      },
    )
    .await;

    assert!(result.is_err());
    assert_eq!(
      websocket_attempts.load(std::sync::atomic::Ordering::Relaxed),
      0
    );
  }
}

#[tokio::test]
async fn websocket_adapter_multiplexes_responses_and_routes_push_frames() {
  let (client_io, server_io) = tokio::io::duplex(64 * 1024);
  let client_socket = WebSocketStream::from_raw_socket(
    client_io,
    tokio_tungstenite::tungstenite::protocol::Role::Client,
    None,
  )
  .await;
  let server_socket = WebSocketStream::from_raw_socket(
    server_io,
    tokio_tungstenite::tungstenite::protocol::Role::Server,
    None,
  )
  .await;
  let transport = WebSocketPunchTransport::from_socket(client_socket);
  let server = tokio::spawn(async move {
    let (mut writer, mut reader) = server_socket.split();
    let mut requests = Vec::new();
    for _ in 0..2 {
      let message = reader.next().await.unwrap().unwrap();
      let Message::Binary(bytes) = message else {
        panic!("client RPCs must be binary WebSocket frames");
      };
      let PunchFrame::Request(request) = serde_json::from_slice(&bytes).unwrap() else {
        panic!("client frame must contain a Punch request");
      };
      requests.push(request);
    }

    let signal = SignalRequest {
      kind: q2q_punch::message::SignalType::Candidate,
      connection_id: "ws-multiplex".into(),
      connect_token: None,
      sdp: None,
      candidate: Some("candidate:ws".into()),
      reason: None,
      accept_token: None,
      signal_key: None,
    };
    writer
      .send(Message::Binary(
        serde_json::to_vec(&PunchFrame::Signal(signal.clone()))
          .unwrap()
          .into(),
      ))
      .await
      .unwrap();

    // Deliberately reply in reverse order to verify request-ID multiplexing.
    for request in requests.into_iter().rev() {
      let response = Response::ok(&request.id, &request.method);
      writer
        .send(Message::Binary(
          serde_json::to_vec(&PunchFrame::Response(response))
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();
    }

    while let Some(Ok(message)) = reader.next().await {
      if matches!(message, Message::Close(_)) {
        let _ = writer.send(Message::Close(None)).await;
        break;
      }
    }
  });

  let first = Request {
    v: RPC_VERSION,
    id: "rpc-first".into(),
    method: "first".into(),
    params: serde_json::Value::Null,
  };
  let second = Request {
    v: RPC_VERSION,
    id: "rpc-second".into(),
    method: "second".into(),
    params: serde_json::Value::Null,
  };
  let (first_response, second_response) =
    tokio::join!(transport.rpc(first), transport.rpc(second),);
  assert_eq!(
    first_response.unwrap().result,
    Some(serde_json::json!("first"))
  );
  assert_eq!(
    second_response.unwrap().result,
    Some(serde_json::json!("second"))
  );

  let received = transport.recv_signal().await.unwrap();
  assert_eq!(received.connection_id, "ws-multiplex");
  assert_eq!(received.candidate.as_deref(), Some("candidate:ws"));
  transport.close().await;
  server.await.unwrap();
}

#[tokio::test]
async fn websocket_reader_fails_explicitly_when_signal_queue_is_full() {
  let (client_io, server_io) = tokio::io::duplex(64 * 1024);
  let client_socket = WebSocketStream::from_raw_socket(
    client_io,
    tokio_tungstenite::tungstenite::protocol::Role::Client,
    None,
  )
  .await;
  let server_socket = WebSocketStream::from_raw_socket(
    server_io,
    tokio_tungstenite::tungstenite::protocol::Role::Server,
    None,
  )
  .await;
  let transport = WebSocketPunchTransport::from_socket(client_socket);
  let server = tokio::spawn(async move {
    let (mut writer, mut reader) = server_socket.split();
    let request = reader.next().await.unwrap().unwrap();
    let Message::Binary(bytes) = request else {
      panic!("client RPC must be binary");
    };
    let PunchFrame::Request(request) = serde_json::from_slice(&bytes).unwrap() else {
      panic!("client frame must contain a Punch request");
    };
    for index in 0..65 {
      let signal = SignalRequest {
        kind: q2q_punch::message::SignalType::Candidate,
        connection_id: format!("queue-{index}"),
        connect_token: None,
        sdp: None,
        candidate: Some(format!("candidate:{index}")),
        reason: None,
        accept_token: None,
        signal_key: None,
      };
      writer
        .send(Message::Binary(
          serde_json::to_vec(&PunchFrame::Signal(signal))
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();
    }
    writer
      .send(Message::Binary(
        serde_json::to_vec(&PunchFrame::Response(Response::ok(
          &request.id,
          &"should-not-be-observed",
        )))
        .unwrap()
        .into(),
      ))
      .await
      .unwrap();
    while let Some(Ok(message)) = reader.next().await {
      if matches!(message, Message::Close(_)) {
        break;
      }
    }
  });

  let request = Request {
    v: RPC_VERSION,
    id: "rpc-with-signal-backlog".into(),
    method: "punch.test".into(),
    params: serde_json::Value::Null,
  };
  let error = tokio::time::timeout(Duration::from_secs(2), transport.rpc(request))
    .await
    .unwrap()
    .unwrap_err();
  assert!(error.to_string().contains("reader is closed"));

  for _ in 0..64 {
    transport.recv_signal().await.unwrap();
  }
  let error = transport.recv_signal().await.unwrap_err();
  assert!(error.to_string().contains("signal queue is full"));
  transport.close().await;
  server.await.unwrap();
}

fn blocked_websocket_transport() -> (
  WebSocketPunchTransport,
  mpsc::Receiver<WsCommand>,
  tokio::task::JoinHandle<()>,
  tokio::task::JoinHandle<()>,
) {
  let (outgoing, commands) = mpsc::channel(1);
  let (signal_tx, signals) = mpsc::channel(1);
  drop(signal_tx);
  let pending = Arc::new(SyncMutex::new(HashMap::new()));
  let reader_task = tokio::spawn(std::future::pending::<()>());
  let writer_task = tokio::spawn(std::future::pending::<()>());
  let transport = WebSocketPunchTransport {
    outgoing,
    pending,
    signals: Mutex::new(signals),
    reader: reader_task.abort_handle(),
    writer: writer_task.abort_handle(),
  };
  (transport, commands, reader_task, writer_task)
}

#[tokio::test]
async fn websocket_rpc_deadline_covers_enqueue_and_cleans_pending_waiter() {
  let (transport, commands, _reader, _writer) = blocked_websocket_transport();
  transport
    .outgoing
    .send(WsCommand::Pong(vec![]))
    .await
    .unwrap();
  let request = Request {
    v: RPC_VERSION,
    id: "rpc-enqueue-timeout".into(),
    method: "punch.test".into(),
    params: serde_json::Value::Null,
  };
  let error = transport
    .rpc_with_timeout(request, Duration::from_millis(20))
    .await
    .unwrap_err();
  assert!(error.to_string().contains("timed out"));
  assert!(lock_pending(&transport.pending).is_empty());
  drop(commands);
}

#[tokio::test]
async fn cancelling_websocket_rpc_removes_pending_waiter() {
  let (transport, commands, reader, writer) = blocked_websocket_transport();
  transport
    .outgoing
    .send(WsCommand::Pong(vec![]))
    .await
    .unwrap();
  let transport = Arc::new(transport);
  let request = Request {
    v: RPC_VERSION,
    id: "rpc-cancelled".into(),
    method: "punch.test".into(),
    params: serde_json::Value::Null,
  };
  let call_transport = transport.clone();
  let call = tokio::spawn(async move {
    call_transport
      .rpc_with_timeout(request, Duration::from_secs(30))
      .await
  });
  tokio::time::timeout(Duration::from_secs(1), async {
    while lock_pending(&transport.pending).is_empty() {
      tokio::task::yield_now().await;
    }
  })
  .await
  .unwrap();
  call.abort();
  let _ = call.await;
  assert!(lock_pending(&transport.pending).is_empty());

  transport
    .close_with_timeout(Duration::from_millis(20))
    .await;
  drop(commands);
  assert!(reader.await.unwrap_err().is_cancelled());
  assert!(writer.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn websocket_close_deadline_aborts_tasks_and_clears_pending_waiters() {
  let (transport, commands, reader, writer) = blocked_websocket_transport();
  transport
    .outgoing
    .send(WsCommand::Pong(vec![]))
    .await
    .unwrap();
  let (sender, waiter) = oneshot::channel();
  lock_pending(&transport.pending).insert("in-flight".into(), sender);

  transport
    .close_with_timeout(Duration::from_millis(20))
    .await;
  assert!(lock_pending(&transport.pending).is_empty());
  assert!(waiter.await.is_err());
  drop(commands);
  assert!(reader.await.unwrap_err().is_cancelled());
  assert!(writer.await.unwrap_err().is_cancelled());
}
