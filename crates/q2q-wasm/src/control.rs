//! Thin control-plane bindings. Host supplies entropy/time and owns all IO.
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use ed25519_dalek::SigningKey;
use q2q_primitives::identity::{
  keypair_from_seed, sign_message, Audience, PeerId, Ticket, TICKET_VERSION,
};
use q2q_punch::message::{
  binding_mac, exit_mac, join_signing_bytes, ExitRequest, HeartbeatRequest, JoinRequest,
};
use wasm_bindgen::prelude::*;

fn invalid() -> JsError {
  JsError::new("invalid control input")
}
fn bounded(value: &str) -> Result<(), JsError> {
  if value.is_empty()
    || value.len() > 128
    || !value
      .bytes()
      .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
  {
    return Err(invalid());
  }
  Ok(())
}
fn key(bytes: &[u8]) -> Result<[u8; 32], JsError> {
  bytes.try_into().map_err(|_| invalid())
}
fn clock(now: u64) -> Result<(), JsError> {
  if now > 9_007_199_254_740_391 {
    return Err(invalid());
  }
  Ok(())
}
#[wasm_bindgen]
pub struct ControlIdentity {
  key: SigningKey,
  peer: PeerId,
}
#[wasm_bindgen]
impl ControlIdentity {
  #[wasm_bindgen(constructor)]
  pub fn new(seed: &[u8]) -> Result<Self, JsError> {
    let (key, peer) = keypair_from_seed(&key(seed)?);
    Ok(Self { key, peer })
  }
  pub fn info(&self) -> String {
    serde_json::json!({"peer_id":self.peer.as_str(),"public_key":B64.encode(self.key.verifying_key().to_bytes())}).to_string()
  }
  fn ticket_body(
    &self,
    punch: Option<String>,
    target: Option<String>,
    hash: Option<Vec<u8>>,
    connection: Option<String>,
    now: u64,
  ) -> Result<String, JsError> {
    clock(now)?;
    let audience = match punch {
      Some(id) => {
        bounded(&id)?;
        Audience::Punch(id)
      }
      None => Audience::Track,
    };
    let is_connect = target.is_some();
    if is_connect && (matches!(audience, Audience::Track) || hash.is_none() || connection.is_none())
    {
      return Err(invalid());
    }
    if !is_connect && hash.is_some() {
      return Err(invalid());
    }
    let target = target
      .map(|id| {
        let peer = PeerId::unchecked(id);
        if !peer.checksum_is_valid() {
          Err(invalid())
        } else {
          Ok(peer)
        }
      })
      .transpose()?;
    let ih = hash.map(|bytes| key(&bytes)).transpose()?;
    if let Some(ref id) = connection {
      bounded(id)?;
    }
    let mut ticket = Ticket {
      v: TICKET_VERSION,
      iss: self.peer.clone(),
      public_key: self.key.verifying_key().to_bytes(),
      iat: now,
      exp: now + if is_connect { 60 } else { 300 },
      aud: audience.to_wire(),
      tgt: target,
      ih,
      cid: connection,
      sig: [0; 64],
    };
    ticket.sign(&self.key);
    Ok(serde_json::to_string(&ticket.to_body())?)
  }
  /// Opaque raw JSON preserves exact numeric wire tokens; it is not a JS ticket encoder.
  pub fn announce(&self, now: u64) -> Result<String, JsError> {
    self.ticket_body(None, None, None, None, now)
  }
  pub fn connect_ticket(
    &self,
    punch: String,
    target: String,
    hash: &[u8],
    connection: String,
    now: u64,
  ) -> Result<String, JsError> {
    self.ticket_body(
      Some(punch),
      Some(target),
      Some(hash.to_vec()),
      Some(connection),
      now,
    )
  }
  pub fn join(&self, punch: String, nonce: String, now: u64) -> Result<String, JsError> {
    let token = self.ticket_body(Some(punch), None, None, Some(nonce), now)?;
    let req = JoinRequest {
      signature: B64.encode(sign_message(&self.key, &join_signing_bytes(&token, now))),
      join_token: token,
      public_key: B64.encode(self.key.verifying_key().to_bytes()),
      timestamp: now,
    };
    Ok(serde_json::to_string(&req)?)
  }
  pub fn track_auth(&self, session_key: &[u8], seq: u64, now: u64) -> Result<String, JsError> {
    clock(now)?;
    Ok(serde_json::json!({"seq":seq,"timestamp":now,"mac":B64.encode(q2q_track::auth::mac_bytes(&key(session_key)?,&self.peer,seq,now))}).to_string())
  }
}
#[wasm_bindgen]
pub fn punch_binding_auth(
  binding_key: &[u8],
  binding_id: String,
  seq: u64,
  now: u64,
  generation: u64,
  exit: bool,
) -> Result<String, JsError> {
  clock(now)?;
  bounded(&binding_id)?;
  let key = key(binding_key)?;
  if exit {
    Ok(serde_json::to_string(&ExitRequest {
      mac: exit_mac(&key, &binding_id, seq, now, generation),
      binding_id,
      seq,
      timestamp: now,
      generation,
    })?)
  } else {
    Ok(serde_json::to_string(&HeartbeatRequest {
      mac: binding_mac(&key, &binding_id, seq, now, generation),
      binding_id,
      seq,
      timestamp: now,
      generation,
    })?)
  }
}

#[wasm_bindgen]
pub fn decode_punch_frame(bytes: &[u8]) -> Result<String, JsError> {
  if bytes.len() > 64 * 1024 {
    return Err(invalid());
  }
  let frame: q2q_punch::transport::PunchFrame = serde_json::from_slice(bytes)?;
  match &frame {
    q2q_punch::transport::PunchFrame::Response(value) if value.v == 1 && value.id.len() <= 128 => {}
    q2q_punch::transport::PunchFrame::Signal(value) => {
      bounded(&value.connection_id)?;
    }
    _ => return Err(invalid()),
  }
  Ok(serde_json::to_string(&frame)?)
}

#[wasm_bindgen]
impl ControlIdentity {
  /// Challenges and SDP come from the host; all transcript encoding remains shared Rust.
  #[allow(clippy::too_many_arguments)]
  pub fn begin_peer(
    &self,
    remote: &[u8],
    initiator: bool,
    hash: &[u8],
    connection: &str,
    nonce: &[u8],
    local_sdp: &str,
    remote_sdp: &str,
  ) -> Result<crate::peer::PeerAuthentication, JsError> {
    bounded(connection)?;
    crate::peer::PeerAuthentication::new(
      self.key.clone(),
      key(remote)?,
      initiator,
      key(hash)?,
      connection.to_owned(),
      key(nonce)?,
      local_sdp,
      remote_sdp,
    )
  }
}

#[wasm_bindgen]
impl ControlIdentity {
  /// Validate an incoming connect ticket before the browser accepts any signal.
  pub fn inspect_connect(
    &self,
    raw: &str,
    punch: &str,
    connection: &str,
    now: u64,
  ) -> Result<String, JsError> {
    if raw.len() > 8192 {
      return Err(invalid());
    }
    clock(now)?;
    bounded(punch)?;
    bounded(connection)?;
    let body: q2q_primitives::identity::TicketBody = serde_json::from_str(raw)?;
    let ticket = body.to_ticket().map_err(|_| invalid())?;
    ticket
      .verify(&Audience::Punch(punch.to_owned()), Some(&self.peer), now)
      .map_err(|_| invalid())?;
    if ticket.cid.as_deref() != Some(connection) || ticket.ih.is_none() {
      return Err(invalid());
    }
    Ok(serde_json::json!({"peer_id":ticket.iss.as_str(),"public_key":B64.encode(ticket.public_key),"info_hash":ticket.ih.unwrap().iter().map(|byte|format!("{byte:02x}")).collect::<String>()}).to_string())
  }
  pub fn peer_id_for_key(&self, public_key: &[u8]) -> Result<String, JsError> {
    Ok(
      q2q_primitives::identity::peer_id_from_public_key(&key(public_key)?)
        .as_str()
        .to_owned(),
    )
  }
  #[allow(clippy::too_many_arguments)]
  pub fn verify_accept(
    &self,
    raw: &str,
    remote: &[u8],
    connection: &str,
    punch: &str,
    hash: &[u8],
    now: u64,
  ) -> Result<(), JsError> {
    if raw.len() > 8192 {
      return Err(invalid());
    }
    clock(now)?;
    let body: q2q_primitives::identity::TicketBody = serde_json::from_str(raw)?;
    let ticket = body.to_ticket().map_err(|_| invalid())?;
    ticket
      .verify(&Audience::Punch(punch.to_owned()), Some(&self.peer), now)
      .map_err(|_| invalid())?;
    let remote = key(remote)?;
    if ticket.iss != q2q_primitives::identity::peer_id_from_public_key(&remote)
      || ticket.public_key != remote
      || ticket.cid.as_deref() != Some(connection)
      || ticket.ih != Some(key(hash)?)
    {
      return Err(invalid());
    }
    Ok(())
  }
}
