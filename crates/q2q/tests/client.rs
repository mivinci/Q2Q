use super::*;

#[tokio::test]
async fn download_concurrency_is_bounded_and_defaults_to_four() {
  let dir = tempfile::tempdir().unwrap();
  for count in [0, 17, usize::MAX] {
    assert!(matches!(
      builder().max_download_peers(count).build().await,
      Err(BuildError::InvalidDownloadPeers)
    ));
  }
  let make = || {
    builder()
      .track_url("http://127.0.0.1:1")
      .identity_file(dir.path().join("identity"))
      .storage_dir(dir.path().join("storage"))
  };
  assert_eq!(make().build().await.unwrap().max_download_peers, 4);
  assert_eq!(
    make()
      .max_download_peers(1)
      .build()
      .await
      .unwrap()
      .max_download_peers,
    1
  );
  assert_eq!(
    make()
      .max_download_peers(16)
      .build()
      .await
      .unwrap()
      .max_download_peers,
    16
  );
}

#[tokio::test]
async fn identity_survives_rebuilding_the_client() {
  let dir = tempfile::tempdir().unwrap();
  let identity_file = dir.path().join("identity");
  let storage_dir = dir.path().join("storage");
  let make_client = || {
    builder()
      .track_url("https://track.example.net")
      .identity_file(&identity_file)
      .storage_dir(&storage_dir)
  };

  let first = make_client().build().await.unwrap();
  let peer_id = first.peer_id().clone();
  let public_key = first.public_key();
  drop(first);

  let second = make_client().build().await.unwrap();
  assert_eq!(second.peer_id(), &peer_id);
  assert_eq!(second.public_key(), public_key);
}

#[tokio::test]
async fn corrupt_identity_is_rejected_without_replacement() {
  let dir = tempfile::tempdir().unwrap();
  let identity_file = dir.path().join("identity");
  std::fs::write(&identity_file, b"broken").unwrap();
  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&identity_file, std::fs::Permissions::from_mode(0o600)).unwrap();
  }

  let result = builder()
    .track_url("https://track.example.net")
    .identity_file(&identity_file)
    .storage_dir(dir.path().join("storage"))
    .build()
    .await;
  assert!(matches!(
    result,
    Err(BuildError::Identity(
      crate::identity::IdentityError::InvalidFormat
    ))
  ));
  assert_eq!(std::fs::read(&identity_file).unwrap(), b"broken");
}
