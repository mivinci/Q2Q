//! Punch Server: the signalling plane.
//!
//! A Punch Server holds standing bindings for the peers assigned to it, and
//! relays signalling from a peer that wants to connect to one bound here. It
//! never issues connection credentials — those are self-signed by the client —
//! and it does not parse the SDP it carries.

pub mod binding;

pub use binding::{Binding, BindingError, Bindings, Generation, UnixSeconds};
