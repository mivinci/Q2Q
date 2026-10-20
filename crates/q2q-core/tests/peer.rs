use super::*;

pub(crate) async fn test_pair(info_hash: [u8; 32]) -> (PeerSession, PeerSession) {
  authenticated_pair(info_hash, false, false).await.unwrap()
}

async fn authenticated_pair(
  info_hash: [u8; 32],
  prefetch: bool,
  corrupt_nonce: bool,
) -> Result<(PeerSession, PeerSession), ControlError> {
  use tokio::sync::{Mutex, mpsc};
  struct Pipe {
    tx: mpsc::Sender<SignalRequest>,
    rx: Mutex<mpsc::Receiver<SignalRequest>>,
  }
  impl SignalChannel for Pipe {
    async fn signal(
      &self,
      request: &SignalRequest,
    ) -> Result<q2q_punch::message::SignalResponse, ControlError> {
      self
        .tx
        .send(request.clone())
        .await
        .map_err(|_| ControlError::Transport("test signal closed".into()))?;
      Ok(q2q_punch::message::SignalResponse {
        ok: true,
        signal_key: None,
        pending: Vec::new(),
      })
    }
    async fn recv_signal(&self) -> Result<SignalRequest, ControlError> {
      self
        .rx
        .lock()
        .await
        .recv()
        .await
        .ok_or_else(|| ControlError::Transport("test signal closed".into()))
    }
  }
  tokio::time::timeout(Duration::from_secs(10), async {
    let (left_tx, left_rx) = mpsc::channel(64);
    let (right_tx, right_rx) = mpsc::channel(64);
    let left_pipe = Pipe {
      tx: left_tx,
      rx: Mutex::new(right_rx),
    };
    let right_pipe = Pipe {
      tx: right_tx,
      rx: Mutex::new(left_rx),
    };
    let mut left = RtcConnection::new(None).await.unwrap();
    let (offer, channels) = left.offer().await.unwrap();
    let mut right = RtcConnection::new(None).await.unwrap();
    let answer = right.answer(offer).await.unwrap();
    left.apply_answer(answer).await.unwrap();
    let directory = tempfile::tempdir().unwrap();
    let left_identity = Identity::load_or_create(&directory.path().join("left.key")).unwrap();
    let right_identity = Identity::load_or_create(&directory.path().join("right.key")).unwrap();
    let left_auth = PeerIdentity {
      local: &left_identity,
      remote_key: right_identity.public_key(),
      role: Role::Initiator,
      info_hash,
      connection_id: "test-peer-pair",
    };
    let right_auth = PeerIdentity {
      local: &right_identity,
      remote_key: left_identity.public_key(),
      role: Role::Responder,
      info_hash,
      connection_id: "test-peer-pair",
    };
    // Each side authenticates as soon as its own negotiation finishes. A
    // barrier before authentication would hide the production opening race.
    let left_ready = async {
      let mut channels = negotiate(&mut left, &left_pipe, "test-peer-pair", Some(channels)).await?;
      if prefetch && channels.early_hello.is_none() {
        loop {
          match channels.control.poll().await {
            Some(DataChannelEvent::OnMessage(message)) => {
              let mut opening = OpeningControl::default();
              opening.observe(
                Some(DataChannelEvent::OnMessage(message)),
                RTCDataChannelState::Open,
              )?;
              channels.early_hello = opening.hello;
              break;
            }
            Some(DataChannelEvent::OnClose) | None => {
              return Err(ControlError::Transport(
                "test identity channel closed".into(),
              ));
            }
            _ => {}
          }
        }
      }
      if corrupt_nonce {
        channels.early_hello.as_mut().expect("prefetched hello")[0] ^= 1;
      }
      left_auth
        .authenticate(&left, &*channels.control, channels.early_hello)
        .await?;
      Ok::<_, ControlError>(channels)
    };
    let right_ready = async {
      let channels = negotiate(&mut right, &right_pipe, "test-peer-pair", None).await?;
      right_auth
        .authenticate(&right, &*channels.control, channels.early_hello)
        .await?;
      Ok::<_, ControlError>(channels)
    };
    let (a, b) = tokio::join!(left_ready, right_ready);
    let left_channels = a?;
    let right_channels = b?;
    Ok((
      PeerSession {
        peer_id: right_identity.peer_id().clone(),
        info_hash,
        connection_id: "test-peer-pair".into(),
        rtc: left,
        control: left_channels.control,
        data: left_channels.data,
      },
      PeerSession {
        peer_id: left_identity.peer_id().clone(),
        info_hash,
        connection_id: "test-peer-pair".into(),
        rtc: right,
        control: right_channels.control,
        data: right_channels.data,
      },
    ))
  })
  .await
  .expect("local test peer negotiation timed out")
}

fn hello_event(nonce: [u8; 32]) -> DataChannelEvent {
  let json =
    serde_json::json!({"type": "identity_hello", "v": 1, "nonce": B64.encode(nonce)}).to_string();
  DataChannelEvent::OnMessage(webrtc::data_channel::RTCDataChannelMessage {
    is_string: true,
    data: bytes::BytesMut::from(json.as_bytes()),
  })
}

#[test]
fn early_hello_is_bounded_and_waits_for_transport_readiness() {
  let mut opening = OpeningControl::default();
  opening
    .observe(Some(hello_event([7; 32])), RTCDataChannelState::Connecting)
    .unwrap();
  assert!(!opening.open);
  assert_eq!(opening.hello, Some([7; 32]));
  opening
    .observe(Some(DataChannelEvent::OnOpen), RTCDataChannelState::Open)
    .unwrap();
  assert!(opening.open);
  assert_eq!(opening.hello, Some([7; 32]));

  let mut already_open = OpeningControl::default();
  already_open
    .observe(Some(hello_event([8; 32])), RTCDataChannelState::Open)
    .unwrap();
  assert!(already_open.open);
  assert_eq!(already_open.hello, Some([8; 32]));

  let mut repeated = OpeningControl::default();
  repeated
    .observe(Some(hello_event([9; 32])), RTCDataChannelState::Connecting)
    .unwrap();
  assert!(
    repeated
      .observe(Some(hello_event([9; 32])), RTCDataChannelState::Connecting)
      .is_err()
  );
  assert!(
    repeated
      .observe(Some(DataChannelEvent::OnClose), RTCDataChannelState::Closed)
      .is_err()
  );
  assert!(
    OpeningControl::default()
      .observe(None, RTCDataChannelState::Closed)
      .is_err()
  );
}

#[tokio::test]
async fn prefetched_hello_completes_mutual_authentication() {
  let (left, right) = authenticated_pair([17; 32], true, false).await.unwrap();
  assert_eq!(left.info_hash(), right.info_hash());
  let (a, b) = tokio::join!(left.close(), right.close());
  a.unwrap();
  b.unwrap();
}

#[tokio::test]
async fn prefetched_hello_still_requires_a_matching_signature() {
  let result = authenticated_pair([18; 32], true, true).await;
  assert!(matches!(result, Err(ControlError::Authentication(_))));
}

#[tokio::test]
async fn prefetched_challenge_cannot_bypass_control_channel_validation() {
  let (left, right) = test_pair([19; 32]).await;
  let directory = tempfile::tempdir().unwrap();
  let identity = Identity::load_or_create(&directory.path().join("other.key")).unwrap();
  let auth = PeerIdentity {
    local: &identity,
    remote_key: [0; 32],
    role: Role::Initiator,
    info_hash: [19; 32],
    connection_id: "wrong-channel",
  };
  let error = auth
    .authenticate(&left.rtc, left.data.as_ref(), Some([1; 32]))
    .await
    .unwrap_err();
  assert!(error.to_string().contains("reliable ordered q2q-control"));
  right.close().await.unwrap();
}
