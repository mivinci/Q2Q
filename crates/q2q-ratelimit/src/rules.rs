//! Rule names, in one place.
//!
//! An unknown rule fails open — the limiter would rather keep serving than take
//! the service down over a typo. Which makes a typo here invisible: the traffic
//! just stops being limited. So the names live here and both services borrow
//! them.

/// Track: announcing writes the index and verifies a signature, so it costs far
/// more than a query.
pub const TRACK_ANNOUNCE: &str = "announce";
pub const TRACK_QUERY: &str = "query";

/// Punch: establishing a binding verifies a signature, so it is the expensive
/// one. Heartbeats are frequent but cheap to reject.
pub const PUNCH_JOIN: &str = "join";
pub const PUNCH_HEARTBEAT: &str = "heartbeat";
/// Waking a peer is the request that actually bothers someone else, so it is
/// limited per file as well as per network.
pub const PUNCH_CONNECT: &str = "connect";
pub const PUNCH_SIGNAL: &str = "signal";
