//! Client SDK for publishing and downloading content on a Q2Q network.
//!
//! The SDK prepares a persistent identity, publishes local files and downloads
//! verified blocks through authenticated WebRTC peer sessions.

mod control;
mod download_store;
mod identity;
mod peer;
mod peer_identity;
mod punch_transport;
pub mod transfer;
mod webrtc_transport;

pub mod client;
pub mod publication;

/// The exact wire shapes used by both the SDK and the services.
pub mod protocol {
  pub use q2q_core::peer_wire as peer;
  pub use q2q_punch::message as punch;
  pub use q2q_track::wire as track;
}
