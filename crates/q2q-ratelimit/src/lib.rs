//! Rate limiting shared by Track and Punch Server.
//!
//! Keyed on network prefix rather than identity: anyone can mint an identity
//! for free, so limiting by `peer_id` would accomplish nothing.

pub mod layer;
pub mod limiter;
pub mod network;
pub mod rules;

pub use layer::{PeerAddr, RateLimitLayer, RateLimitService};
pub use limiter::{Key, Limiter, Rule, Verdict};
pub use network::NetworkKey;
