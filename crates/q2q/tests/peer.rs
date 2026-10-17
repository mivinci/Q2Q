use super::*;

pub(crate) async fn test_pair(info_hash: [u8; 32]) -> (PeerSession, PeerSession) {
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
    let (left_channels, right_channels) = tokio::join!(
      negotiate(&mut left, &left_pipe, "test-peer-pair", Some(channels)),
      negotiate(&mut right, &right_pipe, "test-peer-pair", None),
    );
    let left_channels = left_channels.unwrap();
    let right_channels = right_channels.unwrap();
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
    let (a, b) = tokio::join!(
      left_auth.authenticate(&left, &*left_channels.control),
      right_auth.authenticate(&right, &*right_channels.control)
    );
    a.unwrap();
    b.unwrap();
    (
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
    )
  })
  .await
  .expect("local test peer negotiation timed out")
}
