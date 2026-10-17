use super::*;

#[test]
fn dtls_binding_accepts_identical_fingerprints_and_rejects_ambiguity() {
  let hex = ["AB"; 32].join(":");
  let line = format!("a=fingerprint:sha-256 {hex}\r\n");
  assert_eq!(
    fingerprint(&format!("v=0\r\n{line}m=application\r\n{line}")),
    Ok([0xab; 32])
  );
  assert_eq!(fingerprint(&line.to_lowercase()), Ok([0xab; 32]));
  for invalid in [
    "v=0\r\n".to_owned(),
    line.replace("sha-256", "sha-1"),
    line.replace(&hex, &["AB"; 31].join(":")),
    line.replace(&hex, &["AB"; 33].join(":")),
    line.replace("AB", "GG"),
    format!("{line}{}", line.replace("AB", "CD")),
  ] {
    assert!(fingerprint(&invalid).is_err());
  }
}
