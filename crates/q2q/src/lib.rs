//! Client SDK for publishing and downloading content on a Q2Q network.
//!
//! The SDK prepares a persistent identity, local files and authenticated WebRTC
//! peer sessions. Block transfer is added in the next stage.

mod control;
mod identity;
mod peer;
mod peer_identity;
mod punch_transport;
mod webrtc_transport;

pub mod client;
pub mod publication;

/// The exact wire shapes used by both the SDK and the services.
pub mod protocol {
  pub use q2q_punch::message as punch;
  pub use q2q_track::wire as track;
}
