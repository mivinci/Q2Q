//! Consent-first signalling between two SDK peers.

use crate::control::ControlError;
use crate::identity::Identity;
use crate::peer_identity::PeerIdentity;
use crate::punch_transport::{PunchBinding, PunchEphemeral};
use crate::webrtc_transport::{PeerChannels, RtcConnection, RtcEvent};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use q2q_primitives::identity::{Audience, PeerId, TicketBody};
use q2q_primitives::peer_auth::Role;
use q2q_punch::message::{RejectReason, SignalRequest, SignalType};
use q2q_track::wire::{PeerWire, TurnWire};
use std::sync::Arc;
use std::time::Duration;
use webrtc::data_channel::{DataChannel, DataChannelEvent};
use webrtc::peer_connection::{
  RTCIceCandidateInit, RTCPeerConnectionState, RTCSdpType, RTCSessionDescription,
};

const ACCEPT_TIMEOUT: Duration = Duration::from_secs(10);
const OFFER_TIMEOUT: Duration = Duration::from_secs(10);
const ICE_TIMEOUT: Duration = Duration::from_secs(20);

/// A WebRTC peer whose identity was verified on this DTLS channel. File
/// operations use separate reliable control and unreliable piece channels.
pub struct PeerSession {
  peer_id: PeerId,
  info_hash: [u8; 32],
  connection_id: String,
  rtc: RtcConnection,
  pub(crate) control: Arc<dyn DataChannel>,
  pub(crate) data: Arc<dyn DataChannel>,
}

impl PeerSession {
  pub fn peer_id(&self) -> &PeerId {
    &self.peer_id
  }

  pub fn info_hash(&self) -> &[u8; 32] {
    &self.info_hash
  }

  pub fn connection_id(&self) -> &str {
    &self.connection_id
  }

  pub async fn close(self) -> Result<(), ControlError> {
    self.rtc.close().await.map_err(ControlError::Transport)
  }
}

#[tracing::instrument(skip_all, fields(connection_role = "initiator", connection_id = tracing::field::Empty, local_peer = %identity.peer_id().as_str(), remote_peer = %target.peer_id))]
pub(crate) async fn dial(
  identity: &Identity,
  target: &PeerWire,
  info_hash: [u8; 32],
  turn: &TurnWire,
) -> Result<PeerSession, ControlError> {
  let target_id = PeerId::unchecked(&target.peer_id);
  let target_key = B64
    .decode(&target.public_key)
    .map_err(|_| ControlError::Protocol("target public key is not base64".into()))?;
  let target_key: [u8; 32] = target_key
    .try_into()
    .map_err(|_| ControlError::Protocol("target public key must be 32 bytes".into()))?;
  if q2q_primitives::identity::peer_id_from_public_key(&target_key) != target_id {
    return Err(ControlError::Protocol(
      "target peer id and public key disagree".into(),
    ));
  }
  let channel = PunchEphemeral::connect(&target.punch).await?;
  let connection_id = random_id()?;
  tracing::Span::current().record("connection_id", connection_id.as_str());
  tracing::debug!(%connection_id, "Punch connected; requesting peer acceptance");
  let now = q2q_punch::binding::now();
  let token = identity.connection_ticket(
    &target.punch.punch_id,
    &target_id,
    info_hash,
    &connection_id,
    now,
  );
  let token = serde_json::to_string(&token)
    .map_err(|e| ControlError::Protocol(format!("connect ticket: {e}")))?;
  let mut connect = message(SignalType::Connect, &connection_id);
  connect.connect_token = Some(token.clone());
  channel.signal(&connect).await?;

  let accepted = tokio::time::timeout(ACCEPT_TIMEOUT, channel.recv_signal())
    .await
    .map_err(|_| ControlError::Transport("peer did not accept in time".into()))??;
  if accepted.connection_id != connection_id {
    return Err(ControlError::Protocol(
      "unexpected connection id from Punch".into(),
    ));
  }
  match accepted.kind {
    SignalType::Accept => verify_accept(
      &accepted,
      &target_id,
      &target_key,
      identity.peer_id(),
      info_hash,
      &target.punch.punch_id,
    )?,
    SignalType::Reject if matches!(accepted.reason, Some(RejectReason::Busy)) => {
      return Err(ControlError::Busy);
    }
    SignalType::Reject => {
      return Err(ControlError::Protocol(format!(
        "peer rejected connection: {:?}",
        accepted.reason
      )));
    }
    _ => return Err(ControlError::Protocol("expected peer acceptance".into())),
  }

  tracing::debug!(%connection_id, "Peer accepted; creating WebRTC offer");
  let mut rtc = rtc_connection(turn, &connection_id).await?;
  let (offer, data_channel) = rtc.offer().await.map_err(ControlError::Transport)?;
  let mut offer_message = message(SignalType::Offer, &connection_id);
  offer_message.connect_token = Some(token);
  offer_message.sdp = Some(
    serde_json::to_string(&offer).map_err(|e| ControlError::Protocol(format!("offer SDP: {e}")))?,
  );
  channel.signal(&offer_message).await?;

  let mut pending_candidates = Vec::new();
  let answer = tokio::time::timeout(OFFER_TIMEOUT, async {
    loop {
      let received = channel.recv_signal().await?;
      if received.connection_id != connection_id {
        continue;
      }
      match received.kind {
        SignalType::Answer => break description(&received, RTCSdpType::Answer),
        SignalType::Candidate => pending_candidates.push(candidate(&received)?),
        SignalType::Cancel | SignalType::Reject => {
          return Err(ControlError::Protocol("peer cancelled connection".into()));
        }
        _ => {}
      }
    }
  })
  .await
  .map_err(|_| ControlError::Transport("peer answer timed out".into()))??;
  tracing::debug!(%connection_id, "Received WebRTC answer");
  rtc
    .apply_answer(answer)
    .await
    .map_err(ControlError::Transport)?;
  for candidate in pending_candidates {
    log_candidate("received before answer", &candidate);
    rtc
      .add_candidate(candidate)
      .await
      .map_err(ControlError::Transport)?;
  }
  let channels = negotiate(&mut rtc, &channel, &connection_id, Some(data_channel)).await?;
  PeerIdentity {
    local: identity,
    remote_key: target_key,
    role: Role::Initiator,
    info_hash,
    connection_id: &connection_id,
  }
  .authenticate(&rtc, channels.control.as_ref())
  .await?;
  tracing::debug!("WebRTC peer identity authenticated");
  Ok(PeerSession {
    peer_id: target_id,
    info_hash,
    connection_id,
    rtc,
    control: channels.control,
    data: channels.data,
  })
}

pub(crate) struct PendingPeer {
  identity: Identity,
  initiator: PeerId,
  initiator_key: [u8; 32],
  info_hash: [u8; 32],
  connect: SignalRequest,
  channel: RoutedPunch,
  turn: TurnWire,
}
struct RoutedPunch {
  punch: Arc<PunchBinding>,
  route: crate::signal_router::SignalRoute,
}

pub(crate) async fn incoming(
  identity: &Identity,
  punch: Arc<PunchBinding>,
  info_hash: [u8; 32],
  turn: TurnWire,
) -> Result<PendingPeer, ControlError> {
  loop {
    let peer = incoming_any(identity, punch.clone(), turn.clone()).await?;
    if peer.info_hash == info_hash {
      return Ok(peer);
    }
    peer.reject(RejectReason::NotFound).await?;
  }
}
pub(crate) async fn incoming_any(
  identity: &Identity,
  punch: Arc<PunchBinding>,
  turn: TurnWire,
) -> Result<PendingPeer, ControlError> {
  let request = punch.incoming().await?;
  PendingPeer::validate(identity, punch, turn, request).await
}
impl PendingPeer {
  pub(crate) fn info_hash(&self) -> [u8; 32] {
    self.info_hash
  }
  pub(crate) fn connection_id(&self) -> &str {
    &self.connect.connection_id
  }
  pub(crate) fn peer_id(&self) -> &PeerId {
    &self.initiator
  }
  async fn validate(
    identity: &Identity,
    punch: Arc<PunchBinding>,
    turn: TurnWire,
    request: crate::signal_router::IncomingSignal,
  ) -> Result<Self, ControlError> {
    let connect = request.connect;
    let token = connect
      .connect_token
      .as_deref()
      .ok_or_else(|| ControlError::Protocol("connect lacks ticket".into()))?;
    let body =
      q2q_punch::signal::verify_connect(token, punch.punch_id(), q2q_punch::binding::now())
        .map_err(|e| ControlError::Protocol(format!("connect ticket: {e}")))?;
    let ticket = body
      .to_ticket()
      .map_err(|e| ControlError::Protocol(format!("connect ticket: {e}")))?;
    if ticket.tgt.as_ref() != Some(identity.peer_id())
      || ticket.cid.as_deref() != Some(&connect.connection_id)
    {
      return Err(ControlError::Protocol(
        "connect target or id mismatch".into(),
      ));
    }
    let info_hash = ticket
      .ih
      .ok_or_else(|| ControlError::Protocol("connect lacks content hash".into()))?;
    Ok(Self {
      identity: identity.clone(),
      initiator: ticket.iss,
      initiator_key: ticket.public_key,
      info_hash,
      connect,
      channel: RoutedPunch {
        punch,
        route: request.route,
      },
      turn,
    })
  }
  pub(crate) async fn reject(self, reason: RejectReason) -> Result<(), ControlError> {
    let mut reject = message(SignalType::Reject, &self.connect.connection_id);
    reject.reason = Some(reason);
    self.channel.signal(&reject).await.map(|_| ())
  }
  #[tracing::instrument(skip_all, fields(connection_role = "responder", connection_id = %self.connect.connection_id, local_peer = %self.identity.peer_id().as_str(), remote_peer = %self.initiator.as_str()))]
  pub(crate) async fn accept(self) -> Result<PeerSession, ControlError> {
    tracing::debug!("Accepting incoming peer");
    let connect = &self.connect;
    let info_hash = self.info_hash;
    let now = q2q_punch::binding::now();
    let acceptance = self.identity.connection_ticket(
      self.channel.punch.punch_id(),
      &self.initiator,
      info_hash,
      &connect.connection_id,
      now,
    );
    let mut response = message(SignalType::Accept, &connect.connection_id);
    response.accept_token = Some(
      serde_json::to_string(&acceptance)
        .map_err(|e| ControlError::Protocol(format!("accept ticket: {e}")))?,
    );
    self.channel.signal(&response).await?;

    let offered = tokio::time::timeout(OFFER_TIMEOUT, async {
      loop {
        let offered = self.channel.recv_signal().await?;
        if offered.connection_id == connect.connection_id {
          match offered.kind {
            SignalType::Offer => break Ok(offered),
            SignalType::Cancel => {
              break Err(ControlError::Protocol("peer cancelled connection".into()));
            }
            _ => continue,
          }
        }
        if offered.kind == SignalType::Connect {
          let mut reject = message(SignalType::Reject, &offered.connection_id);
          reject.reason = Some(RejectReason::Busy);
          self.channel.signal(&reject).await?;
        }
      }
    })
    .await
    .map_err(|_| ControlError::Transport("peer offer timed out".into()))??;
    if offered.connect_token != connect.connect_token {
      return Err(ControlError::Protocol("offer ticket changed".into()));
    }
    tracing::debug!("Received WebRTC offer; creating answer");
    let offer = description(&offered, RTCSdpType::Offer)?;
    let mut rtc = rtc_connection(&self.turn, &connect.connection_id).await?;
    let answer = rtc.answer(offer).await.map_err(ControlError::Transport)?;
    let mut answer_message = message(SignalType::Answer, &connect.connection_id);
    answer_message.sdp = Some(
      serde_json::to_string(&answer)
        .map_err(|e| ControlError::Protocol(format!("answer SDP: {e}")))?,
    );
    self.channel.signal(&answer_message).await?;
    let data_channel = negotiate(&mut rtc, &self.channel, &connect.connection_id, None).await?;
    PeerIdentity {
      local: &self.identity,
      remote_key: self.initiator_key,
      role: Role::Responder,
      info_hash: self.info_hash,
      connection_id: &connect.connection_id,
    }
    .authenticate(&rtc, data_channel.control.as_ref())
    .await?;
    Ok(PeerSession {
      peer_id: self.initiator.clone(),
      info_hash: self.info_hash,
      connection_id: connect.connection_id.clone(),
      rtc,
      control: data_channel.control,
      data: data_channel.data,
    })
  }
}

fn verify_accept(
  response: &SignalRequest,
  expected_peer: &PeerId,
  expected_key: &[u8],
  me: &PeerId,
  info_hash: [u8; 32],
  punch_id: &str,
) -> Result<(), ControlError> {
  let raw = response
    .accept_token
    .as_deref()
    .ok_or_else(|| ControlError::Protocol("accept lacks ticket".into()))?;
  let body: TicketBody =
    serde_json::from_str(raw).map_err(|e| ControlError::Protocol(format!("accept ticket: {e}")))?;
  let ticket = body
    .to_ticket()
    .map_err(|e| ControlError::Protocol(format!("accept ticket: {e}")))?;
  ticket
    .verify(
      &Audience::Punch(punch_id.to_owned()),
      Some(me),
      q2q_punch::binding::now(),
    )
    .map_err(|e| ControlError::Protocol(format!("accept ticket: {e}")))?;
  if &ticket.iss != expected_peer
    || ticket.public_key.as_slice() != expected_key
    || ticket.cid.as_deref() != Some(&response.connection_id)
    || ticket.ih != Some(info_hash)
  {
    return Err(ControlError::Protocol(
      "accept identity or content mismatch".into(),
    ));
  }
  Ok(())
}

#[tracing::instrument(skip_all, fields(connection_id = %connection_id))]
async fn rtc_connection(
  turn: &TurnWire,
  connection_id: &str,
) -> Result<RtcConnection, ControlError> {
  let creds = (turn.expires_at > q2q_punch::binding::now()).then_some((
    turn.url.as_str(),
    turn.username.as_str(),
    turn.credential.as_str(),
  ));
  // Never log TURN usernames/passwords or raw URLs containing credentials.
  let endpoint = turn.url.split_once(':').and_then(|(scheme, rest)| {
    url::Url::parse(&format!("{scheme}://{}", rest.trim_start_matches('/'))).ok()
  });
  tracing::debug!(
    turn_enabled = creds.is_some() && !turn.url.is_empty(),
    turn_credentials_valid = turn.expires_at > q2q_punch::binding::now(),
    turn_scheme = endpoint.as_ref().map(|u| u.scheme()),
    turn_host = endpoint.as_ref().and_then(|u| u.host_str()),
    turn_port = endpoint.as_ref().and_then(|u| u.port()),
    "WebRTC ICE configuration"
  );
  RtcConnection::new(creds)
    .await
    .map_err(ControlError::Transport)
}

fn message(kind: SignalType, connection_id: &str) -> SignalRequest {
  SignalRequest {
    kind,
    connection_id: connection_id.to_owned(),
    connect_token: None,
    sdp: None,
    candidate: None,
    reason: None,
    accept_token: None,
    signal_key: None,
  }
}

fn random_id() -> Result<String, ControlError> {
  let mut bytes = [0u8; 16];
  getrandom::fill(&mut bytes).map_err(ControlError::Random)?;
  Ok(crate::control::hex(&bytes))
}

fn description(
  signal: &SignalRequest,
  kind: RTCSdpType,
) -> Result<RTCSessionDescription, ControlError> {
  let raw = signal
    .sdp
    .as_deref()
    .ok_or_else(|| ControlError::Protocol("signal lacks SDP".into()))?;
  let desc: RTCSessionDescription =
    serde_json::from_str(raw).map_err(|e| ControlError::Protocol(format!("invalid SDP: {e}")))?;
  if desc.sdp_type != kind {
    return Err(ControlError::Protocol("unexpected SDP type".into()));
  }
  Ok(desc)
}

fn candidate(signal: &SignalRequest) -> Result<RTCIceCandidateInit, ControlError> {
  let raw = signal
    .candidate
    .as_deref()
    .ok_or_else(|| ControlError::Protocol("candidate signal is empty".into()))?;
  serde_json::from_str(raw)
    .map_err(|e| ControlError::Protocol(format!("invalid ICE candidate: {e}")))
}

trait SignalChannel {
  async fn signal(
    &self,
    request: &SignalRequest,
  ) -> Result<q2q_punch::message::SignalResponse, ControlError>;
  async fn recv_signal(&self) -> Result<SignalRequest, ControlError>;
}

impl SignalChannel for RoutedPunch {
  async fn signal(
    &self,
    request: &SignalRequest,
  ) -> Result<q2q_punch::message::SignalResponse, ControlError> {
    self.punch.signal(request).await
  }
  async fn recv_signal(&self) -> Result<SignalRequest, ControlError> {
    self.route.recv().await
  }
}

impl SignalChannel for PunchEphemeral {
  async fn signal(
    &self,
    request: &SignalRequest,
  ) -> Result<q2q_punch::message::SignalResponse, ControlError> {
    PunchEphemeral::signal(self, request).await
  }
  async fn recv_signal(&self) -> Result<SignalRequest, ControlError> {
    PunchEphemeral::recv_signal(self).await
  }
}

#[tracing::instrument(skip_all, fields(connection_id = %connection_id))]
async fn negotiate(
  rtc: &mut RtcConnection,
  signal: &impl SignalChannel,
  connection_id: &str,
  channels: Option<PeerChannels>,
) -> Result<PeerChannels, ControlError> {
  let (mut control, mut data) = match channels {
    Some(channels) => (Some(channels.control), Some(channels.data)),
    None => (None, Some(rtc.data_channel())),
  };
  let mut connected = false;
  let mut control_open = false;
  let mut data_open = false;
  tokio::time::timeout(ICE_TIMEOUT, async {
    loop {
      tokio::select! {
        inbound = signal.recv_signal() => {
          let inbound = inbound?;
          if inbound.connection_id != connection_id {
            if inbound.kind == SignalType::Connect {
              let mut reject = message(SignalType::Reject, &inbound.connection_id);
              reject.reason = Some(RejectReason::Busy);
              signal.signal(&reject).await?;
            }
            continue;
          }
          match inbound.kind {
            SignalType::Candidate => {
              let candidate = candidate(&inbound)?;
              log_candidate("received", &candidate);
              rtc.add_candidate(candidate).await.map_err(ControlError::Transport)?;
            },
            SignalType::Cancel | SignalType::Reject => return Err(ControlError::Protocol("peer cancelled connection".into())),
            _ => {}
          }
        }
        event = rtc.next_event() => match event {
          Some(RtcEvent::Candidate(candidate)) => {
            log_candidate("sending", &candidate);
            let mut outbound = message(SignalType::Candidate, connection_id);
            outbound.candidate = Some(serde_json::to_string(&candidate)
              .map_err(|e| ControlError::Protocol(format!("ICE candidate: {e}")))?);
            signal.signal(&outbound).await?;
          }
          Some(RtcEvent::Channel(inbound)) => {
            let label = inbound.label().await.map_err(|e| ControlError::Transport(e.to_string()))?;
            let slot = match label.as_str() {
              "q2q-control" => &mut control,
              "q2q-data" => &mut data,
              _ => return Err(ControlError::Protocol("unexpected data channel label".into())),
            };
            if slot.is_some() {
              return Err(ControlError::Protocol("unexpected additional data channel".into()));
            }
            *slot = Some(inbound);
          }
          Some(RtcEvent::State(state)) => {
            tracing::debug!(?state, "WebRTC connection state");
            match state {
              RTCPeerConnectionState::Connected => connected = true,
              RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed => {
                return Err(ControlError::Transport("WebRTC connection failed".into()));
              }
              _ => {}
            }
          },
          None => return Err(ControlError::Transport("WebRTC event channel closed".into())),
        },
        event = async {
          match &control {
            Some(channel) => channel.poll().await,
            None => std::future::pending().await,
          }
        }, if !control_open => match event {
          Some(DataChannelEvent::OnOpen) => { control_open = true; tracing::debug!("Control data channel opened"); },
          Some(DataChannelEvent::OnMessage(_)) => return Err(ControlError::Authentication("received data before channel open".into())),
          Some(DataChannelEvent::OnClose) | None => return Err(ControlError::Transport("WebRTC data channel closed".into())),
          _ => {}
        },
        event = async {
          match &data {
            Some(channel) => channel.poll().await,
            None => std::future::pending().await,
          }
        }, if !data_open => match event {
          Some(DataChannelEvent::OnOpen) => { data_open = true; tracing::debug!("Piece data channel opened"); },
          Some(DataChannelEvent::OnMessage(_)) => return Err(ControlError::Authentication("received data before channel open".into())),
          Some(DataChannelEvent::OnClose) | None => return Err(ControlError::Transport("WebRTC data channel closed".into())),
          _ => {}
        },
      }
      if connected && control_open && data_open {
        let data = data.ok_or_else(|| ControlError::Protocol("missing data channel".into()))?;
        if data.ordered().await.map_err(|e| ControlError::Transport(e.to_string()))?
          || data.max_retransmits().await.map_err(|e| ControlError::Transport(e.to_string()))? != Some(0)
          || data.max_packet_life_time().await.map_err(|e| ControlError::Transport(e.to_string()))?.is_some()
        {
          return Err(ControlError::Protocol("expected unreliable unordered data channel".into()));
        }
        return Ok(PeerChannels {
          control: control.ok_or_else(|| ControlError::Protocol("missing control channel".into()))?,
          data,
        });
      }
    }
  })
  .await
  .map_err(|_| {
    tracing::debug!(connected, control_open, data_open, timeout_seconds = ICE_TIMEOUT.as_secs(), "WebRTC negotiation timed out");
    ControlError::Transport("WebRTC negotiation timed out".into())
  })?
}

#[cfg(test)]
#[path = "../tests/peer.rs"]
mod test_support;
#[cfg(test)]
pub(crate) use test_support::test_pair;

// Log only routing fields: the raw candidate can also contain ICE credentials.
pub(crate) fn log_candidate(direction: &str, candidate: &RTCIceCandidateInit) {
  let fields: Vec<_> = candidate.candidate.split_whitespace().collect();
  let kind = fields.windows(2).find(|p| p[0] == "typ").map(|p| p[1]);
  tracing::debug!(
    direction,
    candidate_type = kind,
    protocol = fields.get(2),
    address = fields.get(4),
    port = fields.get(5),
    "ICE candidate"
  );
}
