//! Opt-in native publication for the browser end-to-end suite. Only the seed and
//! shutdown command use stdio; discovery/signalling/bytes use real services.
use crate::client;
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use std::io::{BufRead, Write};
use std::time::Duration;
#[tokio::test]
#[ignore = "requires local Track/Punch and a Playwright browser"]
async fn browser_publisher_bridge() {
  let track = std::env::var("Q2Q_BROWSER_TRACK").expect("local test Track URL");
  let remote = std::env::var("Q2Q_BROWSER_ALLOW_REMOTE").as_deref() == Ok("1");
  assert!(
    track.starts_with("http://127.0.0.1:") || remote && track.starts_with("https://"),
    "remote publication requires explicit Q2Q_BROWSER_ALLOW_REMOTE=1 and HTTPS"
  );
  let rate: u64 = std::env::var("Q2Q_BROWSER_RATE")
    .unwrap_or_else(|_| "524288".into())
    .parse()
    .unwrap();
  let root = tempfile::tempdir().unwrap();
  let size = 3 * 262144 + 17;
  let bytes: Vec<u8> = (0..size)
    .map(|i| ((i * 31 + i / 262144) % 251) as u8)
    .collect();
  let path = root.path().join("browser-p2p.bin");
  std::fs::write(&path, &bytes).unwrap();
  let client = client::builder()
    .track_url(track)
    .identity_file(root.path().join("identity"))
    .storage_dir(root.path().join("state"))
    .upload_rate_limit(rate)
    .max_upload_peers(4)
    .build()
    .await
    .unwrap();
  let publication = client.publish_file(&path).await.unwrap();
  assert!(publication.seed().cdn_list.is_empty() && publication.seed().proof_list.is_empty());
  println!(
    "Q2Q_BROWSER_SEED {}",
    B64.encode(publication.seed().encode())
  );
  let mut external = publication.seed().clone();
  external.leaf_hashes = None;
  println!(
    "Q2Q_BROWSER_EXTERNAL_SEED {}",
    B64.encode(external.encode())
  );
  std::io::stdout().flush().unwrap();
  let stop = tokio::task::spawn_blocking(|| {
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).unwrap();
    line
  });
  let line = tokio::time::timeout(Duration::from_secs(if remote { 300 } else { 90 }), stop)
    .await
    .unwrap()
    .unwrap();
  assert_eq!(line.trim(), "close");
  publication.close().await.unwrap();
  client.connect().await.unwrap().close().await.unwrap();
}
