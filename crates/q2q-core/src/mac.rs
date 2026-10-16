//! HMAC, shared by Track and Punch Server.
//!
//! Both use it to turn an asymmetric handshake into a cheap symmetric one: a
//! ticket is signed once, after which every follow-up message is MAC'd.

use hmac::{Hmac, KeyInit, Mac};
use sha1::Sha1;
use sha2::Sha256;

/// HMAC takes a key of any length, so this cannot actually fail. The
/// expectation is only there because the crate hands back a `Result`, and this
/// is the one place in the workspace that assumes so.
macro_rules! start {
  ($hash:ty, $key:expr) => {
    Hmac::<$hash>::new_from_slice($key).expect("HMAC accepts a key of any length")
  };
}

/// HMAC-SHA-256 over the concatenated parts.
pub fn sha256(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
  let mut mac = start!(Sha256, key);
  for part in parts {
    mac.update(part);
  }
  mac.finalize().into_bytes().into()
}

/// HMAC-SHA-1 over the concatenated parts. Only coturn's REST mechanism wants
/// this one; everything else we sign is SHA-256.
pub fn sha1(key: &[u8], parts: &[&[u8]]) -> [u8; 20] {
  let mut mac = start!(Sha1, key);
  for part in parts {
    mac.update(part);
  }
  mac.finalize().into_bytes().into()
}

/// Constant-time comparison, so a reply cannot be used to find where the first
/// differing byte was.
pub fn verify(expected: [u8; 32], key: &[u8], parts: &[&[u8]]) -> bool {
  let mut mac = start!(Sha256, key);
  for part in parts {
    mac.update(part);
  }
  mac.verify_slice(&expected).is_ok()
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parts_are_concatenated_in_order() {
    let a = sha256(&[1u8; 32], &[b"ab", b"c"]);
    let b = sha256(&[1u8; 32], &[b"abc"]);
    assert_eq!(a, b);
    assert_ne!(a, sha256(&[1u8; 32], &[b"acb"]));
  }

  #[test]
  fn verification_is_exact() {
    let mac = sha256(&[2u8; 32], &[b"x"]);
    assert!(verify(mac, &[2u8; 32], &[b"x"]));
    assert!(!verify(mac, &[2u8; 32], &[b"y"]));
  }

  #[test]
  fn any_key_length_is_accepted() {
    // the reason this module can exist at all
    assert_ne!(sha256(&[], &[b"x"]), sha256(&[0u8; 200], &[b"x"]));
  }
}
