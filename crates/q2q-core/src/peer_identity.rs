//! Mutual identity confirmation on the already-open WebRTC DataChannel.

use crate::control::ControlError;
use crate::identity::Identity;
use crate::webrtc_transport::RtcConnection;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use q2q_primitives::identity::verify_signature;
use q2q_primitives::peer_auth::IdentityFrame as Frame;
use q2q_primitives::peer_auth::{MAX_IDENTITY_FRAME as MAX_FRAME, Role, Transcript};
use std::time::Duration;
use webrtc::data_channel::{DataChannel, DataChannelEvent, RTCDataChannelMessage};

const VERSION: u32 = 1;
const TIMEOUT: Duration = Duration::from_secs(10);

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
    early_hello: Option<[u8; 32]>,
  ) -> Result<(), ControlError> {
    let result = tokio::time::timeout(TIMEOUT, self.exchange(rtc, channel, early_hello))
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
    early_hello: Option<[u8; 32]>,
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
    let remote_nonce = match early_hello {
      Some(nonce) => nonce,
      None => receive(channel).await?.hello_nonce().map_err(auth_error)?,
    };
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
    let signature = receive(channel)
      .await?
      .proof_signature()
      .map_err(auth_error)?;
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

/// Validate and retain only the first challenge, never business data or a proof.
/// This does not authenticate the peer: the signature exchange still follows.
pub(crate) fn early_hello(message: &RTCDataChannelMessage) -> Result<[u8; 32], ControlError> {
  if !message.is_string {
    return Err(auth_error("identity frames must be text messages"));
  }
  parse(&message.data)?.hello_nonce().map_err(auth_error)
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
  Frame::decode(bytes).map_err(auth_error)
}

fn auth_error(detail: impl Into<String>) -> ControlError {
  ControlError::Authentication(detail.into())
}

#[cfg(test)]
#[path = "../tests/peer_identity.rs"]
mod tests;
