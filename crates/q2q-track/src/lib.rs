//! Track: the discovery and session service.
//!
//! Track answers one question — "who has this content?" — and hands back just
//! enough to reach them: a peer id, its public key, and the Punch Server it is
//! attached to. It never issues identity tickets (clients sign their own) and
//! never stores connection state.

pub mod auth;
#[cfg(feature = "redis")]
pub mod redis;
pub mod state;
