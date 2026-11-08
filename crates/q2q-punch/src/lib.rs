//! Punch Server: the signalling plane.
//!
//! A Punch Server holds standing bindings for the peers assigned to it, and
//! relays signalling from a peer that wants to connect to one bound here. It
//! never issues connection credentials — those are self-signed by the client —
//! and it does not parse the SDP it carries.

pub type UnixSeconds = u64;
/// Bumped on ICE restart to invalidate heartbeats from an older mapping.
pub type Generation = u64;

#[cfg(feature = "server")]
pub mod binding;
pub mod endpoints;
pub mod message;
#[cfg(feature = "server")]
pub mod signal;
pub mod transport;

#[cfg(feature = "server")]
pub use binding::{Binding, BindingError, Bindings};
pub use message::{
  ExitRequest, HeartbeatRequest, HeartbeatResponse, JoinError, JoinRequest, JoinResponse, Method,
  Request, Response, SignalResponse,
};
#[cfg(feature = "server")]
pub use signal::{
  Connection, ConnectionState, Connections, RejectReason, SignalError, SignalRequest, SignalType,
};
pub use transport::PunchFrame;
