use super::*;

#[tokio::test]
async fn cancelling_a_signal_wait_preserves_a_partly_received_quic_push() {
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
  let config = quinn::ServerConfig::with_crypto(Arc::new(
    quinn::crypto::rustls::QuicServerConfig::try_from(crypto).unwrap(),
  ));
  let server = quinn::Endpoint::server(config, "127.0.0.1:0".parse().unwrap()).unwrap();
  let assignment = PunchWire {
    punch_id: "test-punch".into(),
    url: format!("quic://{}", server.local_addr().unwrap()),
    public_key: B64.encode(key.public_key_raw()),
  };
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
