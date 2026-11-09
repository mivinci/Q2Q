//! Opt-in Playwright bridge. Uses the real native negotiator and identity exchange.
//! The loopback JSON socket carries test signalling only; bytes use WebRTC.
use super::*;

use q2q_primitives::peer_auth::Role;
use q2q_primitives::peer_wire::{Control, Piece};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::{
  TcpListener,
  tcp::{OwnedReadHalf, OwnedWriteHalf},
};
use tokio::sync::Mutex;

struct Bridge {
  read: Mutex<Lines<BufReader<OwnedReadHalf>>>,
  write: Mutex<OwnedWriteHalf>,
}
impl Bridge {
  async fn send(&self, value: Value) {
    self
      .write
      .lock()
      .await
      .write_all(format!("{value}\n").as_bytes())
      .await
      .unwrap();
  }
  async fn read(&self) -> Value {
    let line = self
      .read
      .lock()
      .await
      .next_line()
      .await
      .unwrap()
      .expect("test bridge disconnected");
    serde_json::from_str(&line).unwrap()
  }
}
impl SignalChannel for Bridge {
  async fn signal(
    &self,
    request: &SignalRequest,
  ) -> Result<q2q_punch::message::SignalResponse, ControlError> {
    self
      .send(json!({"type":"candidate","candidate":request.candidate}))
      .await;
    Ok(q2q_punch::message::SignalResponse {
      ok: true,
      signal_key: None,
      pending: vec![],
    })
  }
  async fn recv_signal(&self) -> Result<SignalRequest, ControlError> {
    let value = self.read().await;
    assert_eq!(value["type"], "candidate");
    let mut request = message(SignalType::Candidate, "native-browser");
    request.candidate = Some(value["candidate"].to_string());
    Ok(request)
  }
}

#[tokio::test]
#[ignore = "requires a Playwright browser; npm run test:rtc --workspace @q2q/client-web"]
async fn browser_rtc_bridge() {
  assert_eq!(std::env::var("Q2Q_BROWSER_RTC").as_deref(), Ok("1"));
  tokio::time::timeout(Duration::from_secs(40), async {
    let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
    println!("Q2Q_BROWSER_RTC {}",listener.local_addr().unwrap().port());
    let (socket,_)=listener.accept().await.unwrap();let (read,write)=socket.into_split();
    let bridge=Bridge {read:Mutex::new(BufReader::new(read).lines()),write:Mutex::new(write)};
    let root=tempfile::tempdir().unwrap();let identity=Identity::load_or_create(&root.path().join("identity")).unwrap();
    bridge.send(json!({"type":"identity","public_key":B64.encode(identity.public_key())})).await;
    let init=bridge.read().await;assert_eq!(init["type"],"init");
    let remote:[u8;32]=B64.decode(init["public_key"].as_str().unwrap()).unwrap().try_into().unwrap();
    let initiator=init["initiator"].as_bool().unwrap();let mut rtc=RtcConnection::new(None).await.unwrap();
    let channels=if initiator {
      let (offer,channels)=rtc.offer().await.unwrap();bridge.send(json!({"type":"offer","description":offer})).await;
      let reply=bridge.read().await;assert_eq!(reply["type"],"answer");rtc.apply_answer(serde_json::from_value(reply["description"].clone()).unwrap()).await.unwrap();Some(channels)
    } else {
      let offer=bridge.read().await;assert_eq!(offer["type"],"offer");
      let answer=rtc.answer(serde_json::from_value(offer["description"].clone()).unwrap()).await.unwrap();
      bridge.send(json!({"type":"answer","description":answer})).await;None
    };
    let channels=negotiate(&mut rtc,&bridge,"native-browser",channels).await.unwrap();
    crate::peer_identity::PeerIdentity {local:&identity,remote_key:remote,role:if initiator {Role::Initiator}else{Role::Responder},info_hash:[3;32],connection_id:"native-browser"}
      .authenticate(&rtc,channels.control.as_ref(),channels.early_hello).await.unwrap();
    bridge.send(json!({"type":"authenticated"})).await;
    let mut control_done=false;let mut piece_done=false;
    while !control_done||!piece_done {
      tokio::select! {
        event=channels.control.poll(),if !control_done=>if let Some(DataChannelEvent::OnMessage(message))=event {
          assert!(message.is_string);assert!(matches!(Control::decode(&message.data).unwrap(),Control::Have {block_index:u64::MAX}));
          channels.control.send_text(std::str::from_utf8(&message.data).unwrap()).await.unwrap();control_done=true;
        },
        event=channels.data.poll(),if !piece_done=>if let Some(DataChannelEvent::OnMessage(message))=event {
          assert!(!message.is_string);let piece=Piece::decode(&message.data).unwrap();assert_eq!(piece.block_index,u64::MAX);assert_eq!(piece.piece_offset,1024);
          assert_eq!(piece.data,(0..1024).map(|i|(i%251)as u8).collect::<Vec<_>>());
          channels.data.send(message.data).await.unwrap();piece_done=true;
        },
      }
    }
    bridge.send(json!({"type":"echoed"})).await;
    // Do not close before SCTP actually delivers the echoed unreliable piece.
    assert_eq!(bridge.read().await["type"],"close");rtc.close().await.unwrap();
  }).await.expect("browser/native bridge deadline");
}
