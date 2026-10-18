//! Q2Q core: identity, seed, Merkle proofs and local storage, all network-free.
//!
//! This crate never opens a network connection; the only I/O is local disk in
//! the store module. Transport, wire codec and scheduling live elsewhere and
//! call into it.
//!
//! - `bencode` — deterministic encoding, the basis of `info_hash` and signatures
//! - `merkle` — tree, proofs and verification (RFC 6962 shape)
//! - `seed` — info dict, `info_hash`, magnet links
//! - `seed_builder` — incremental byte-stream hashing and seed construction
//! - `identity` — Ed25519 keys, `peer_id`, self-signed tickets
//! - `mac` — HMAC-SHA-256, the cheap follow-up once a ticket has been signed
//! - `block` — one block's state and piece bitmap, no I/O
//! - `file` — in-place writes, progress and leaf-cache persistence
//!
//! Default features keep local storage and system random key generation enabled.
//! Disable `storage` and `random` for compute-only WebAssembly consumers.

pub mod bencode;
pub mod bencode_serde;
pub mod block;
pub mod cache;
#[cfg(feature = "storage")]
pub mod file;
pub mod identity;
pub mod mac;
pub mod merkle;
pub mod peer_auth;
pub mod peer_wire;
pub mod seed;
pub mod seed_builder;
