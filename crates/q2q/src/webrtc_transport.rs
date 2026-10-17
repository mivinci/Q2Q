//! Native WebRTC peer connection used by the SDK's transfer layer.

use std::sync::Arc;
use tokio::sync::mpsc;
use webrtc::data_channel::{DataChannel, RTCDataChannelInit};
use webrtc::peer_connection::{
  PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCConfigurationBuilder,
  RTCIceCandidateInit, RTCIceServer, RTCPeerConnectionIceEvent, RTCPeerConnectionState,
  RTCSessionDescription,
};

#[derive(Clone)]
struct Events {
  candidates: mpsc::UnboundedSender<RTCIceCandidateInit>,
  channels: mpsc::UnboundedSender<Arc<dyn DataChannel>>,
  states: mpsc::UnboundedSender<RTCPeerConnectionState>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for Events {
  async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
    if let Ok(candidate) = event.candidate.to_json() {
      let _ = self.candidates.send(candidate);
    }
  }

  async fn on_data_channel(&self, channel: Arc<dyn DataChannel>) {
    let _ = self.channels.send(channel);
  }

  async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
    let _ = self.states.send(state);
  }
}

pub(crate) enum RtcEvent {
  Candidate(RTCIceCandidateInit),
  Channel(Arc<dyn DataChannel>),
  State(RTCPeerConnectionState),
}

pub(crate) struct RtcConnection {
  peer: Arc<dyn PeerConnection>,
  candidates: mpsc::UnboundedReceiver<RTCIceCandidateInit>,
  channels: mpsc::UnboundedReceiver<Arc<dyn DataChannel>>,
  states: mpsc::UnboundedReceiver<RTCPeerConnectionState>,
}

impl RtcConnection {
  pub(crate) async fn new(turn: Option<(&str, &str, &str)>) -> Result<Self, String> {
    let (candidate_tx, candidates) = mpsc::unbounded_channel();
    let (channel_tx, channels) = mpsc::unbounded_channel();
    let (state_tx, states) = mpsc::unbounded_channel();
    let mut config = RTCConfigurationBuilder::new();
    if let Some((url, username, credential)) = turn.filter(|(url, _, _)| !url.is_empty()) {
      config = config.with_ice_servers(vec![RTCIceServer {
        urls: vec![url.to_owned()],
        username: username.to_owned(),
        credential: credential.to_owned(),
      }]);
    }
    let peer = PeerConnectionBuilder::new()
      .with_configuration(config.build())
      .with_handler(Arc::new(Events {
        candidates: candidate_tx,
        channels: channel_tx,
        states: state_tx,
      }))
      .with_udp_addrs(vec!["0.0.0.0:0".to_owned()])
      .build()
      .await
      .map_err(|e| e.to_string())?;
    Ok(Self {
      peer: Arc::new(peer),
      candidates,
      channels,
      states,
    })
  }

  pub(crate) async fn offer(
    &self,
  ) -> Result<(RTCSessionDescription, Arc<dyn DataChannel>), String> {
    let channel = self
      .peer
      .create_data_channel("q2q-blocks", Some(RTCDataChannelInit::default()))
      .await
      .map_err(|e| e.to_string())?;
    let offer = self
      .peer
      .create_offer(None)
      .await
      .map_err(|e| e.to_string())?;
    self
      .peer
      .set_local_description(offer.clone())
      .await
      .map_err(|e| e.to_string())?;
    Ok((offer, channel))
  }

  pub(crate) async fn answer(
    &self,
    offer: RTCSessionDescription,
  ) -> Result<RTCSessionDescription, String> {
    self
      .peer
      .set_remote_description(offer)
      .await
      .map_err(|e| e.to_string())?;
    let answer = self
      .peer
      .create_answer(None)
      .await
      .map_err(|e| e.to_string())?;
    self
      .peer
      .set_local_description(answer.clone())
      .await
      .map_err(|e| e.to_string())?;
    Ok(answer)
  }

  pub(crate) async fn apply_answer(&self, answer: RTCSessionDescription) -> Result<(), String> {
    self
      .peer
      .set_remote_description(answer)
      .await
      .map_err(|e| e.to_string())
  }

  pub(crate) async fn add_candidate(&self, candidate: RTCIceCandidateInit) -> Result<(), String> {
    self
      .peer
      .add_ice_candidate(candidate)
      .await
      .map_err(|e| e.to_string())
  }

  pub(crate) async fn next_event(&mut self) -> Option<RtcEvent> {
    tokio::select! {
      candidate = self.candidates.recv() => candidate.map(RtcEvent::Candidate),
      channel = self.channels.recv() => channel.map(RtcEvent::Channel),
      state = self.states.recv() => state.map(RtcEvent::State),
    }
  }

  pub(crate) async fn close(&self) -> Result<(), String> {
    self.peer.close().await.map_err(|e| e.to_string())
  }

  pub(crate) async fn fingerprints(&self) -> Result<([u8; 32], [u8; 32]), String> {
    let local = self
      .peer
      .local_description()
      .await
      .ok_or("missing local SDP")?;
    let remote = self
      .peer
      .remote_description()
      .await
      .ok_or("missing remote SDP")?;
    Ok((fingerprint(&local.sdp)?, fingerprint(&remote.sdp)?))
  }
}

impl Drop for RtcConnection {
  fn drop(&mut self) {
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
      let peer = self.peer.clone();
      runtime.spawn(async move {
        let _ = peer.close().await;
      });
    }
  }
}

fn fingerprint(sdp: &str) -> Result<[u8; 32], String> {
  let mut found = None;
  for line in sdp.lines() {
    let Some(value) = line.strip_prefix("a=fingerprint:") else {
      continue;
    };
    let mut fields = value.split_whitespace();
    let algorithm = fields.next().ok_or("missing fingerprint algorithm")?;
    let value = fields.next().ok_or("missing fingerprint value")?;
    if !algorithm.eq_ignore_ascii_case("sha-256") || fields.next().is_some() {
      return Err("identity confirmation requires a SHA-256 DTLS fingerprint".into());
    }
    let mut hash = [0u8; 32];
    let mut octets = value.split(':');
    for byte in &mut hash {
      let octet = octets.next().ok_or("short DTLS fingerprint")?;
      if octet.len() != 2 {
        return Err("invalid DTLS fingerprint octet".into());
      }
      *byte = u8::from_str_radix(octet, 16).map_err(|_| "invalid DTLS fingerprint hex")?;
    }
    if octets.next().is_some() {
      return Err("long DTLS fingerprint".into());
    }
    if found.is_some_and(|previous| previous != hash) {
      return Err("conflicting DTLS fingerprints".into());
    }
    found = Some(hash);
  }
  found.ok_or_else(|| "missing DTLS fingerprint".into())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn dtls_binding_accepts_identical_fingerprints_and_rejects_ambiguity() {
    let hex = ["AB"; 32].join(":");
    let line = format!("a=fingerprint:sha-256 {hex}\r\n");
    assert_eq!(
      fingerprint(&format!("v=0\r\n{line}m=application\r\n{line}")),
      Ok([0xab; 32])
    );
    assert_eq!(fingerprint(&line.to_lowercase()), Ok([0xab; 32]));
    for invalid in [
      "v=0\r\n".to_owned(),
      line.replace("sha-256", "sha-1"),
      line.replace(&hex, &["AB"; 31].join(":")),
      line.replace(&hex, &["AB"; 33].join(":")),
      line.replace("AB", "GG"),
      format!("{line}{}", line.replace("AB", "CD")),
    ] {
      assert!(fingerprint(&invalid).is_err());
    }
  }
}
