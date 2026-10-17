//! Client SDK for publishing and downloading content on a Q2Q network.
//!
//! The SDK prepares a persistent identity and local files for seeding.
//! Discovery, signalling and peer transfer are added in later stages.

mod control;
mod identity;
mod peer;
mod punch_transport;
mod webrtc_transport;

pub mod client;
pub mod publication;

/// The exact wire shapes used by both the SDK and the services.
pub mod protocol {
  pub use q2q_punch::message as punch;
  pub use q2q_track::wire as track;
}
