use super::*;

#[test]
fn malformed_or_business_frames_are_rejected_during_identity_confirmation() {
  for raw in [
    r#"{"type":"request","block":0}"#,
    r#"{"type":"identity_hello","v":1}"#,
    r#"{"type":"identity_hello","v":1,"nonce":"AA==","nonce":"AA=="}"#,
    r#"{"type":"identity_proof","v":1,"signature":"AA==","extra":0}"#,
  ] {
    assert!(matches!(
      parse(raw.as_bytes()),
      Err(ControlError::Authentication(_))
    ));
  }
  assert!(decode::<32>("AA==").is_err());
  assert!(decode::<64>(&B64.encode([0u8; 32])).is_err());
  assert!(decode::<32>("not base64!").is_err());
  assert!(parse(&vec![b' '; MAX_FRAME + 1]).is_err());
}

#[test]
fn unknown_versions_and_proof_before_challenge_are_rejected() {
  let nonce = B64.encode([1u8; 32]);
  let signature = B64.encode([2u8; 64]);
  assert!(
    Frame::Hello {
      v: 2,
      nonce: nonce.clone()
    }
    .hello_nonce()
    .is_err()
  );
  assert!(
    Frame::Proof {
      v: VERSION,
      signature: signature.clone()
    }
    .hello_nonce()
    .is_err()
  );
  assert!(
    Frame::Hello { v: VERSION, nonce }
      .proof_signature()
      .is_err()
  );
  assert!(Frame::Proof { v: 2, signature }.proof_signature().is_err());
}
