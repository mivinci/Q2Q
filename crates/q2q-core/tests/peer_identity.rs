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

#[test]
fn early_control_frame_accepts_only_a_bounded_text_hello() {
  use webrtc::data_channel::RTCDataChannelMessage;
  let nonce = [13; 32];
  let raw =
    serde_json::json!({"type": "identity_hello", "v": 1, "nonce": B64.encode(nonce)}).to_string();
  let message = |text: bool, data: &[u8]| RTCDataChannelMessage {
    is_string: text,
    data: bytes::BytesMut::from(data),
  };
  assert_eq!(early_hello(&message(true, raw.as_bytes())).unwrap(), nonce);
  for (text, raw) in [
    (false, raw.into_bytes()),
    (true, vec![b' '; MAX_FRAME + 1]),
    (true, br#"{"type":"request","block_index":0}"#.to_vec()),
    (
      true,
      serde_json::json!({"type":"identity_proof", "v":1, "signature": B64.encode([0; 64])})
        .to_string()
        .into_bytes(),
    ),
  ] {
    assert!(matches!(
      early_hello(&message(text, &raw)),
      Err(ControlError::Authentication(_))
    ));
  }
}
