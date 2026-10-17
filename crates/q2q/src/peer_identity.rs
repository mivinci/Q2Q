//! Mutual identity confirmation on the already-open WebRTC DataChannel.

use crate::control::ControlError;
use crate::identity::Identity;
use crate::webrtc_transport::RtcConnection;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use q2q_core::identity::verify_signature;
use q2q_core::peer_auth::{Role, Transcript};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use webrtc::data_channel::{DataChannel, DataChannelEvent};

const VERSION: u32 = 1;
const MAX_FRAME: usize = 512;
const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
enum Frame {
  #[serde(rename = "identity_hello")]
  Hello { v: u32, nonce: String },
  #[serde(rename = "identity_proof")]
  Proof { v: u32, signature: String },
}

impl Frame {
  fn hello_nonce(self) -> Result<[u8; 32], ControlError> {
    match self {
      Self::Hello { v: VERSION, nonce } => decode(&nonce),
      _ => Err(auth_error("expected identity_hello version 1")),
    }
  }

  fn proof_signature(self) -> Result<[u8; 64], ControlError> {
    match self {
      Self::Proof {
        v: VERSION,
        signature,
      } => decode(&signature),
      _ => Err(auth_error("expected identity_proof version 1")),
    }
  }
}

pub(crate) struct PeerIdentity<'a> {
  pub local: &'a Identity,
  pub remote_key: [u8; 32],
  pub role: Role,
  pub info_hash: [u8; 32],
  pub connection_id: &'a str,
}

impl PeerIdentity<'_> {
  pub(crate) async fn authenticate(
    &self,
    rtc: &RtcConnection,
    channel: &dyn DataChannel,
  ) -> Result<(), ControlError> {
    let result = tokio::time::timeout(TIMEOUT, self.exchange(rtc, channel))
      .await
      .map_err(|_| ControlError::Authentication("identity confirmation timed out".into()))
      .and_then(|result| result);
    if result.is_err() {
      let _ = rtc.close().await;
    }
    result
  }

  async fn exchange(
    &self,
    rtc: &RtcConnection,
    channel: &dyn DataChannel,
  ) -> Result<(), ControlError> {
    if channel
      .label()
      .await
      .map_err(|e| auth_error(e.to_string()))?
      != "q2q-control"
      || !channel
        .ordered()
        .await
        .map_err(|e| auth_error(e.to_string()))?
      || channel
        .max_packet_life_time()
        .await
        .map_err(|e| auth_error(e.to_string()))?
        .is_some()
      || channel
        .max_retransmits()
        .await
        .map_err(|e| auth_error(e.to_string()))?
        .is_some()
    {
      return Err(auth_error(
        "expected the reliable ordered q2q-control channel",
      ));
    }
    let (local_fingerprint, remote_fingerprint) = rtc
      .fingerprints()
      .await
      .map_err(ControlError::Authentication)?;
    let mut local_nonce = [0u8; 32];
    getrandom::fill(&mut local_nonce).map_err(ControlError::Random)?;
    send(
      channel,
      &Frame::Hello {
        v: VERSION,
        nonce: B64.encode(local_nonce),
      },
    )
    .await?;
    let remote_nonce = receive(channel).await?.hello_nonce()?;
    let (
      initiator_key,
      responder_key,
      initiator_nonce,
      responder_nonce,
      initiator_fingerprint,
      responder_fingerprint,
    ) = match self.role {
      Role::Initiator => (
        self.local.public_key(),
        self.remote_key,
        local_nonce,
        remote_nonce,
        local_fingerprint,
        remote_fingerprint,
      ),
      Role::Responder => (
        self.remote_key,
        self.local.public_key(),
        remote_nonce,
        local_nonce,
        remote_fingerprint,
        local_fingerprint,
      ),
    };
    let transcript = Transcript {
      initiator_key,
      responder_key,
      info_hash: self.info_hash,
      connection_id: self.connection_id,
      initiator_nonce,
      responder_nonce,
      initiator_fingerprint,
      responder_fingerprint,
    };
    let signature = self.local.sign(&transcript.signing_bytes(self.role));
    send(
      channel,
      &Frame::Proof {
        v: VERSION,
        signature: B64.encode(signature),
      },
    )
    .await?;
    let signature = receive(channel).await?.proof_signature()?;
    let remote_role = match self.role {
      Role::Initiator => Role::Responder,
      Role::Responder => Role::Initiator,
    };
    if !verify_signature(
      &self.remote_key,
      &transcript.signing_bytes(remote_role),
      &signature,
    ) {
      return Err(auth_error(
        "peer identity signature does not match this channel",
      ));
    }
    Ok(())
  }
}

async fn send(channel: &dyn DataChannel, frame: &Frame) -> Result<(), ControlError> {
  let json =
    serde_json::to_string(frame).map_err(|e| auth_error(format!("identity encoding: {e}")))?;
  channel
    .send_text(&json)
    .await
    .map_err(|e| auth_error(e.to_string()))
}

async fn receive(channel: &dyn DataChannel) -> Result<Frame, ControlError> {
  loop {
    match channel.poll().await {
      Some(DataChannelEvent::OnMessage(message)) => {
        if !message.is_string {
          return Err(auth_error("identity frames must be text messages"));
        }
        return parse(&message.data);
      }
      Some(DataChannelEvent::OnClose) | None => return Err(auth_error("identity channel closed")),
      _ => {}
    }
  }
}

fn parse(bytes: &[u8]) -> Result<Frame, ControlError> {
  if bytes.len() > MAX_FRAME {
    return Err(auth_error("identity frame is too large"));
  }
  serde_json::from_slice(bytes).map_err(|e| auth_error(format!("invalid identity frame: {e}")))
}

fn decode<const N: usize>(raw: &str) -> Result<[u8; N], ControlError> {
  B64
    .decode(raw)
    .map_err(|_| auth_error("identity field is not base64"))?
    .try_into()
    .map_err(|_| auth_error("identity field has invalid length"))
}

fn auth_error(detail: impl Into<String>) -> ControlError {
  ControlError::Authentication(detail.into())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn malformed_or_business_frames_are_rejected_during_identity_confirmation() {
    for raw in [
      r#"{"type":"request","block":0}"#,
      r#"{"type":"identity_hello","v":1}"#,
      r#"{"type":"identity_hello","v":1,"nonce":"AA==","nonce":"AA=="}"#,
      r#"{"type":"identity_proof","v":1,"signature":"AA==","extra":0}"#,
    ] {
      assert!(matches!(
        parse(raw.as_bytes()),
        Err(ControlError::Authentication(_))
      ));
    }
    assert!(decode::<32>("AA==").is_err());
    assert!(decode::<64>(&B64.encode([0u8; 32])).is_err());
    assert!(decode::<32>("not base64!").is_err());
    assert!(parse(&vec![b' '; MAX_FRAME + 1]).is_err());
  }

  #[test]
  fn unknown_versions_and_proof_before_challenge_are_rejected() {
    let nonce = B64.encode([1u8; 32]);
    let signature = B64.encode([2u8; 64]);
    assert!(
      Frame::Hello {
        v: 2,
        nonce: nonce.clone()
      }
      .hello_nonce()
      .is_err()
    );
    assert!(
      Frame::Proof {
        v: VERSION,
        signature: signature.clone()
      }
      .hello_nonce()
      .is_err()
    );
    assert!(
      Frame::Hello { v: VERSION, nonce }
        .proof_signature()
        .is_err()
    );
    assert!(Frame::Proof { v: 2, signature }.proof_signature().is_err());
  }
}
