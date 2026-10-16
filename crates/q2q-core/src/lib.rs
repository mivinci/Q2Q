//! Q2Q core: identity, seed, Merkle proofs and local storage, all network-free.
//!
//! This crate never opens a network connection; the only I/O is local disk in
//! the store module. Transport, wire codec and scheduling live elsewhere and
//! call into it.
//!
//! - `bencode` — deterministic encoding, the basis of `info_hash` and signatures
//! - `merkle` — tree, proofs and verification (RFC 6962 shape)
//! - `seed` — info dict, `info_hash`, magnet links
//! - `identity` — Ed25519 keys, `peer_id`, self-signed tickets

pub mod bencode;
pub mod identity;
pub mod merkle;
pub mod seed;
