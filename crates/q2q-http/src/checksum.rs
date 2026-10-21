use crate::Error;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;

/// Expected file identity supplied by trusted application metadata.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Checksum {
  Sha1([u8; 20]),
  Sha256([u8; 32]),
}
fn decode<const N: usize>(hex: &str) -> Result<[u8; N], Error> {
  if hex.len() != N * 2 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
    return Err(Error::Invalid(
      "checksum must be hexadecimal with the correct length",
    ));
  }
  let mut result = [0; N];
  for (i, byte) in result.iter_mut().enumerate() {
    *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).expect("checked ASCII hex");
  }
  Ok(result)
}
impl Checksum {
  pub fn sha1(hex: &str) -> Result<Self, Error> {
    Ok(Self::Sha1(decode(hex)?))
  }
  pub fn sha256(hex: &str) -> Result<Self, Error> {
    Ok(Self::Sha256(decode(hex)?))
  }
  pub(crate) fn verify(
    &self,
    mut reader: impl Read,
    mut progress: impl FnMut(u64) -> Result<(), Error>,
  ) -> Result<(), Error> {
    let mut sha1 = sha1::Sha1::new();
    let mut sha256 = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    let mut count = 0;
    loop {
      progress(count)?;
      let n = reader.read(&mut buffer)?;
      if n == 0 {
        break;
      }
      match self {
        Self::Sha1(_) => sha1.update(&buffer[..n]),
        Self::Sha256(_) => sha256.update(&buffer[..n]),
      }
      count += n as u64;
    }
    let valid = match self {
      Self::Sha1(expected) => sha1.finalize().as_slice() == expected,
      Self::Sha256(expected) => sha256.finalize().as_slice() == expected,
    };
    if valid { Ok(()) } else { Err(Error::Integrity) }
  }
}
