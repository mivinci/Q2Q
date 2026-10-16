//! Bucketing peers by network rather than by address.
//!
//! Rate limiting has to be keyed on the network, not the host: an attacker
//! rotating through addresses inside their own prefix would otherwise get a
//! fresh bucket each time. The prefix lengths follow the usual split — /24 for
//! IPv4, /64 for IPv6, the latter being the smallest block a site is normally
//! delegated.

use std::net::IpAddr;

/// The prefix an address belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NetworkKey {
  V4([u8; 3]),
  V6([u8; 8]),
}

impl NetworkKey {
  pub fn of(ip: IpAddr) -> Self {
    match ip {
      IpAddr::V4(v4) => {
        let o = v4.octets();
        NetworkKey::V4([o[0], o[1], o[2]])
      }
      IpAddr::V6(v6) => {
        let o = v6.octets();
        NetworkKey::V6([o[0], o[1], o[2], o[3], o[4], o[5], o[6], o[7]])
      }
    }
  }
}

impl std::fmt::Display for NetworkKey {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      NetworkKey::V4(p) => write!(f, "{}.{}.{}.0/24", p[0], p[1], p[2]),
      NetworkKey::V6(p) => {
        let u16s: Vec<String> = p
          .chunks(2)
          .map(|c| format!("{:x}{:02x}", c[0], c[1]))
          .collect();
        write!(f, "{}::/64", u16s.join(":"))
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::net::Ipv6Addr;

  #[test]
  fn v4_addresses_in_the_same_slash24_share_a_key() {
    let a: IpAddr = "203.0.113.7".parse().unwrap();
    let b: IpAddr = "203.0.113.200".parse().unwrap();
    let c: IpAddr = "203.0.114.7".parse().unwrap();
    assert_eq!(NetworkKey::of(a), NetworkKey::of(b));
    assert_ne!(NetworkKey::of(a), NetworkKey::of(c));
  }

  #[test]
  fn v6_addresses_in_the_same_slash64_share_a_key() {
    let a = IpAddr::V6(
      "2001:db8:abcd:0012:0000:0000:0000:0001"
        .parse::<Ipv6Addr>()
        .unwrap(),
    );
    let b = IpAddr::V6(
      "2001:db8:abcd:0012:ffff:ffff:ffff:ffff"
        .parse::<Ipv6Addr>()
        .unwrap(),
    );
    let c = IpAddr::V6(
      "2001:db8:abcd:0013:0000:0000:0000:0001"
        .parse::<Ipv6Addr>()
        .unwrap(),
    );
    assert_eq!(NetworkKey::of(a), NetworkKey::of(b));
    assert_ne!(NetworkKey::of(a), NetworkKey::of(c));
  }

  #[test]
  fn keys_display_as_prefixes() {
    let v4: IpAddr = "203.0.113.7".parse().unwrap();
    assert_eq!(NetworkKey::of(v4).to_string(), "203.0.113.0/24");
  }
}
