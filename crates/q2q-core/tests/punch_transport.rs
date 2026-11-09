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
    endpoints: vec![format!("quic://{}", server.local_addr().unwrap())]
      .try_into()
      .unwrap(),
    public_key: B64.encode(key.public_key_raw()),
  };
  (server, assignment)
}

#[test]
fn websocket_endpoint_preserves_independent_authority_path_and_query() {
  let assignment = PunchWire {
    punch_id: "test-punch".into(),
    endpoints: vec![
      "quic://punch.invalid:4433".into(),
      "wss://proxy.invalid:8443/signal/ws?region=hk".into(),
    ]
    .try_into()
    .unwrap(),
    public_key: B64.encode([0; 32]),
  };

  assert_eq!(
    websocket_url(&assignment).unwrap(),
    "wss://proxy.invalid:8443/signal/ws?region=hk"
  );
}

#[test]
fn websocket_endpoint_never_guesses_from_quic() {
  let assignment = PunchWire {
    punch_id: "test-punch".into(),
    endpoints: vec!["quic://punch.example:4433".into()].try_into().unwrap(),
    public_key: B64.encode([0; 32]),
  };

  assert!(websocket_url(&assignment).is_err());
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
async fn quic_heartbeat_uses_a_reliable_bidirectional_stream() {
  let (server, assignment) = punch_server(None);
  let (release_server, wait_for_release) = tokio::sync::oneshot::channel();
  let server_task = tokio::spawn(async move {
    let connection = server.accept().await.unwrap().await.unwrap();
    let (mut send, mut recv) = tokio::time::timeout(Duration::from_secs(2), connection.accept_bi())
      .await
      .expect("heartbeat should use a QUIC stream")
      .unwrap();
    let bytes = recv.read_to_end(MAX_RESPONSE_BYTES).await.unwrap();
    let request: Request = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(request.method, "punch.heartbeat");
    send
      .write_all(
        &serde_json::to_vec(&Response::ok(
          &request.id,
          &HeartbeatResponse { expires_at: 123 },
        ))
        .unwrap(),
      )
      .await
      .unwrap();
    send.finish().unwrap();
    let _ = wait_for_release.await;
  });

  let (endpoint, connection) = connect_quic(&assignment).await.unwrap();
  let transport = QuicPunchTransport {
    endpoint,
    signals: SignalReceiver::new(connection.clone()),
    connection,
  };
  let request = Request {
    v: RPC_VERSION,
    id: "heartbeat-stream-test".into(),
    method: "punch.heartbeat".into(),
    params: serde_json::Value::Null,
  };
  let response = transport.heartbeat(request).await.unwrap();
  assert_eq!(response.id, "heartbeat-stream-test");
  assert!(response.error.is_none());
  let _ = release_server.send(());
  transport.close().await;
  server_task.await.unwrap();
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

fn factory_assignment() -> PunchWire {
  PunchWire {
    punch_id: "test".into(),
    public_key: B64.encode([0; 32]),
    endpoints: vec![
      "wss://proxy.invalid:8443/signal?region=hk".into(),
      "quic://punch.invalid:4433".into(),
    ]
    .try_into()
    .unwrap(),
  }
}
#[tokio::test]
async fn transport_factory_prefers_quic_and_does_not_start_websocket() {
  let calls = AtomicUsize::new(0);
  let transport = connect_transport_with(
    &factory_assignment(),
    |_| async { Ok(fake_transport(PunchTransportKind::Quic)) },
    |_| {
      calls.fetch_add(1, Ordering::Relaxed);
      async { Ok(fake_transport(PunchTransportKind::WebSocket)) }
    },
  )
  .await
  .unwrap();
  assert_eq!(transport.kind(), PunchTransportKind::Quic);
  assert_eq!(calls.load(Ordering::Relaxed), 0);
}
#[tokio::test]
async fn transport_factory_falls_back_only_to_the_explicit_endpoint() {
  let calls = AtomicUsize::new(0);
  let transport = connect_transport_with(
    &factory_assignment(),
    |_| async { Err(ControlError::Transport("UDP unavailable".into())) },
    |one| {
      calls.fetch_add(1, Ordering::Relaxed);
      assert_eq!(
        one.endpoints.as_slice(),
        ["wss://proxy.invalid:8443/signal?region=hk"]
      );
      async { Ok(fake_transport(PunchTransportKind::WebSocket)) }
    },
  )
  .await
  .unwrap();
  assert_eq!(transport.kind(), PunchTransportKind::WebSocket);
  assert_eq!(calls.load(Ordering::Relaxed), 1);
}
#[tokio::test]
async fn transport_factory_does_not_hide_auth_or_protocol_errors_with_fallback() {
  for error in [
    ControlError::Authentication("wrong pinned key".into()),
    ControlError::Protocol("malformed response".into()),
    ControlError::Remote {
      service: "Punch",
      status: 0,
      code: Some("unauthorized".into()),
      detail: None,
    },
  ] {
    let calls = AtomicUsize::new(0);
    let mut error = Some(error);
    assert!(
      connect_transport_with(
        &factory_assignment(),
        |_| {
          let error = error.take().unwrap();
          async move { Err(error) }
        },
        |_| {
          calls.fetch_add(1, Ordering::Relaxed);
          async { Ok(fake_transport(PunchTransportKind::WebSocket)) }
        },
      )
      .await
      .is_err()
    );
    assert_eq!(calls.load(Ordering::Relaxed), 0);
  }
}
#[tokio::test]
async fn endpoint_factory_orders_all_quic_then_wss_then_loopback_ws() {
  let assignment = PunchWire {
    punch_id: "test".into(),
    public_key: B64.encode([0; 32]),
    endpoints: vec![
      "ws://127.0.0.1:8080/ws".into(),
      "wss://proxy.invalid/ws".into(),
      "quic://b.invalid:4433".into(),
      "quic://a.invalid:4433".into(),
    ]
    .try_into()
    .unwrap(),
  };
  let seen = std::sync::Mutex::new(Vec::new());
  let transport = connect_transport_with(
    &assignment,
    |one| {
      seen
        .lock()
        .unwrap()
        .push(one.endpoints.as_slice()[0].clone());
      async { Err(ControlError::Transport("UDP unavailable".into())) }
    },
    |one| {
      let url = one.endpoints.as_slice()[0].clone();
      seen.lock().unwrap().push(url.clone());
      async move {
        if url.starts_with("ws:") {
          Ok(fake_transport(PunchTransportKind::WebSocket))
        } else {
          Err(ControlError::Transport("WSS unavailable".into()))
        }
      }
    },
  )
  .await
  .unwrap();
  assert_eq!(transport.kind(), PunchTransportKind::WebSocket);
  assert_eq!(
    *seen.lock().unwrap(),
    vec![
      "quic://a.invalid:4433",
      "quic://b.invalid:4433",
      "wss://proxy.invalid/ws",
      "ws://127.0.0.1:8080/ws"
    ]
  );
}

#[tokio::test(start_paused = true)]
async fn endpoint_fallback_has_one_total_budget() {
  let calls = AtomicUsize::new(0);
  let result = connect_transport_with(
    &factory_assignment(),
    |_| async {
      tokio::time::sleep(Duration::from_secs(15)).await;
      Err(ControlError::Transport("UDP unavailable".into()))
    },
    |_| {
      calls.fetch_add(1, Ordering::Relaxed);
      async {
        tokio::time::sleep(Duration::from_secs(15)).await;
        Ok(fake_transport(PunchTransportKind::WebSocket))
      }
    },
  )
  .await;
  assert!(result.is_err());
  assert_eq!(calls.load(Ordering::Relaxed), 1);
}

#[tokio::test]
#[allow(clippy::result_large_err)] // tungstenite's handshake callback fixes this error type.
async fn websocket_only_endpoint_uses_its_explicit_path_and_query() {
  let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
  let address = listener.local_addr().unwrap();
  let server = tokio::spawn(async move {
    let (socket, _) = listener.accept().await.unwrap();
    let mut socket = tokio_tungstenite::accept_hdr_async(
      socket,
      |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response| {
        assert_eq!(request.uri().to_string(), "/signal/punch?region=hk");
        Ok(response)
      },
    )
    .await
    .unwrap();
    while let Some(Ok(message)) = socket.next().await {
      if matches!(message, Message::Close(_)) {
        break;
      }
    }
  });
  let assignment = PunchWire {
    punch_id: "test".into(),
    public_key: B64.encode([0; 32]),
    endpoints: vec![format!("ws://{address}/signal/punch?region=hk")]
      .try_into()
      .unwrap(),
  };
  let transport = connect_transport(&assignment).await.unwrap();
  assert_eq!(transport.kind(), PunchTransportKind::WebSocket);
  transport.close().await;
  server.await.unwrap();
}

#[tokio::test]
async fn wss_rejects_self_signed_certificate_even_if_its_key_matches_track() {
  let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap();
  let cert = rcgen::CertificateParams::new(vec!["localhost".into()])
    .unwrap()
    .self_signed(&key)
    .unwrap();
  let config = rustls::ServerConfig::builder_with_provider(Arc::new(
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
  let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
  let address = listener.local_addr().unwrap();
  let server = tokio::spawn(async move {
    let (socket, _) = listener.accept().await.unwrap();
    let _ = tokio_rustls::TlsAcceptor::from(Arc::new(config))
      .accept(socket)
      .await;
  });
  let assignment = PunchWire {
    punch_id: "test".into(),
    public_key: B64.encode(key.public_key_raw()),
    endpoints: vec![format!("wss://localhost:{}/signal", address.port())]
      .try_into()
      .unwrap(),
  };
  assert!(matches!(
    connect_transport(&assignment).await,
    Err(ControlError::Authentication(_))
  ));
  server.await.unwrap();
}

#[tokio::test]
async fn proxy_authorization_rejection_does_not_try_another_endpoint() {
  use tokio::io::{AsyncReadExt, AsyncWriteExt};
  for status in [401, 403] {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
      let (mut socket, _) = listener.accept().await.unwrap();
      let mut input = Vec::new();
      let mut bytes = [0; 1024];
      while !input.ends_with(b"\r\n\r\n") {
        let n = socket.read(&mut bytes).await.unwrap();
        assert!(n > 0);
        input.extend_from_slice(&bytes[..n]);
        assert!(input.len() < 8192);
      }
      socket
        .write_all(
          format!("HTTP/1.1 {status} Rejected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .as_bytes(),
        )
        .await
        .unwrap();
      drop(socket);
      assert!(
        tokio::time::timeout(Duration::from_millis(150), listener.accept())
          .await
          .is_err(),
        "authorization rejection must not fall back"
      );
    });
    let assignment = PunchWire {
      punch_id: "test".into(),
      public_key: B64.encode([0; 32]),
      endpoints: vec![format!("ws://{address}/a"), format!("ws://{address}/b")]
        .try_into()
        .unwrap(),
    };
    assert!(matches!(
      connect_transport(&assignment).await,
      Err(ControlError::Authentication(_))
    ));
    server.await.unwrap();
  }
}

#[tokio::test]
async fn cancelling_websocket_handshake_closes_the_socket() {
  use tokio::io::AsyncReadExt;
  let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
  let address = listener.local_addr().unwrap();
  let assignment = PunchWire {
    punch_id: "test".into(),
    public_key: B64.encode([0; 32]),
    endpoints: vec![format!("ws://{address}/hanging")].try_into().unwrap(),
  };
  let task = tokio::spawn(async move { connect_transport(&assignment).await });
  let (mut socket, _) = listener.accept().await.unwrap();
  let mut input = [0; 4096];
  assert!(socket.read(&mut input).await.unwrap() > 0);
  task.abort();
  assert!(task.await.err().unwrap().is_cancelled());
  assert_eq!(
    tokio::time::timeout(Duration::from_secs(1), socket.read(&mut input))
      .await
      .unwrap()
      .unwrap(),
    0
  );
}

#[tokio::test]
async fn wrong_quic_identity_does_not_attempt_the_websocket_endpoint() {
  let (server, mut assignment) = punch_server(None);
  let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
  let mut endpoints = assignment.endpoints.as_slice().to_vec();
  endpoints.push(format!("ws://{}/fallback", listener.local_addr().unwrap()));
  assignment.endpoints = endpoints.try_into().unwrap();
  assignment.public_key = B64.encode([42; 32]);
  let quic = tokio::spawn(async move {
    let _ = server.accept().await.unwrap().await;
  });
  assert!(matches!(
    connect_transport(&assignment).await,
    Err(ControlError::Authentication(_))
  ));
  assert!(
    tokio::time::timeout(Duration::from_millis(100), listener.accept())
      .await
      .is_err()
  );
  quic.await.unwrap();
}

#[tokio::test]
async fn quic_join_loss_falls_back_with_a_fresh_verified_ticket() {
  let (server, mut assignment) = punch_server(None);
  let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
  let mut endpoints = assignment.endpoints.as_slice().to_vec();
  endpoints.push(format!(
    "ws://{}/signal?region=local",
    listener.local_addr().unwrap()
  ));
  assignment.endpoints = endpoints.try_into().unwrap();
  let quic = tokio::spawn(async move {
    let connection = server.accept().await.unwrap().await.unwrap();
    let (_send, mut recv) = connection.accept_bi().await.unwrap();
    let request: Request =
      serde_json::from_slice(&recv.read_to_end(MAX_RESPONSE_BYTES).await.unwrap()).unwrap();
    let join: JoinRequest = serde_json::from_value(request.params).unwrap();
    q2q_punch::message::verify_join(&join, "test-punch", q2q_punch::binding::now()).unwrap();
    connection.close(0u32.into(), b"lost response");
    join.join_token
  });
  let websocket = tokio::spawn(async move {
    let (socket, _) = listener.accept().await.unwrap();
    let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
    let Message::Binary(bytes) = socket.next().await.unwrap().unwrap() else {
      panic!("binary request required")
    };
    let PunchFrame::Request(request) = serde_json::from_slice(&bytes).unwrap() else {
      panic!("join required")
    };
    let join: JoinRequest = serde_json::from_value(request.params).unwrap();
    q2q_punch::message::verify_join(&join, "test-punch", q2q_punch::binding::now()).unwrap();
    let response = PunchFrame::Response(Response::ok(
      &request.id,
      &JoinResponse {
        binding_id: "new-binding".into(),
        binding_key: B64.encode([8; 32]),
        expires_at: q2q_punch::binding::now() + 300,
      },
    ));
    socket
      .send(Message::Binary(
        serde_json::to_vec(&response).unwrap().into(),
      ))
      .await
      .unwrap();
    while let Some(Ok(message)) = socket.next().await {
      if matches!(message, Message::Close(_)) {
        break;
      }
    }
    join.join_token
  });
  let directory = tempfile::tempdir().unwrap();
  let identity = Identity::load_or_create(&directory.path().join("identity")).unwrap();
  let (transport, joined) = connect_and_join(&identity, &assignment).await.unwrap();
  assert_eq!(transport.kind(), PunchTransportKind::WebSocket);
  assert_eq!(joined.binding_id, "new-binding");
  transport.close().await;
  let first: serde_json::Value = serde_json::from_str(&quic.await.unwrap()).unwrap();
  let second: serde_json::Value = serde_json::from_str(&websocket.await.unwrap()).unwrap();
  assert_ne!(first["cid"], second["cid"]);
  assert_eq!(first["iss"], second["iss"]);
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
      let sent = writer
        .send(Message::Binary(
          serde_json::to_vec(&PunchFrame::Signal(signal))
            .unwrap()
            .into(),
        ))
        .await
        .is_ok();
      if !sent {
        break;
      }
    }
    let _ = writer
      .send(Message::Binary(
        serde_json::to_vec(&PunchFrame::Response(Response::ok(
          &request.id,
          &"should-not-be-observed",
        )))
        .unwrap()
        .into(),
      ))
      .await;
    while let Some(Ok(message)) = reader.next().await {
      if matches!(message, Message::Close(_)) {
        let _ = writer.send(Message::Close(None)).await;
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
  assert!(error.to_string().contains("signal queue is full"));

  let heartbeat = Request {
    v: RPC_VERSION,
    id: "heartbeat-after-overflow".into(),
    method: "punch.heartbeat".into(),
    params: serde_json::Value::Null,
  };
  let error = tokio::time::timeout(Duration::from_secs(2), transport.heartbeat(heartbeat))
    .await
    .unwrap()
    .unwrap_err();
  assert!(error.to_string().contains("signal queue is full"));

  let rpc = Request {
    v: RPC_VERSION,
    id: "rpc-after-overflow".into(),
    method: "punch.test".into(),
    params: serde_json::Value::Null,
  };
  let error = transport
    .rpc_with_timeout(rpc, Duration::from_millis(50))
    .await
    .unwrap_err();
  assert!(error.to_string().contains("signal queue is full"));

  let error = transport.recv_signal().await.unwrap_err();
  assert!(error.to_string().contains("signal queue is full"));
  tokio::time::timeout(Duration::from_secs(2), server)
    .await
    .unwrap()
    .unwrap();
  assert!(transport.outgoing.is_closed());
  transport.close().await;
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
  let (failure_tx, failure) = tokio::sync::watch::channel(None);
  let reader_task = tokio::spawn(std::future::pending::<()>());
  let writer_task = tokio::spawn(std::future::pending::<()>());
  let transport = WebSocketPunchTransport {
    outgoing,
    pending,
    signals: Mutex::new(signals),
    failure_tx,
    failure,
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
