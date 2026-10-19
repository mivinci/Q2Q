//! Shared local HTTP fixture for unit and public SDK integration tests.
#![allow(dead_code)] // Individual test targets use different parts of this fixture.

use crate::client::Client as SdkClient;
use q2q_primitives::merkle;
use q2q_primitives::seed::{DEFAULT_BLOCK_SIZE, Info, Seed};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::task::JoinSet;

#[derive(Clone, Debug)]
pub(super) struct Request {
  pub(super) path: String,
  pub(super) range: Option<String>,
  pub(super) identity: bool,
  pub(super) referer: Option<String>,
}
pub(super) struct Reply {
  pub(super) status: u16,
  pub(super) range: Option<String>,
  pub(super) encoding: Option<&'static str>,
  pub(super) length: Option<usize>,
  pub(super) body: Vec<u8>,
  pub(super) hang: bool,
  pub(super) location: Option<String>,
}
impl Reply {
  pub(super) fn body(body: Vec<u8>) -> Self {
    Self {
      status: 200,
      range: None,
      encoding: None,
      length: Some(body.len()),
      body,
      hang: false,
      location: None,
    }
  }
}
pub(super) struct Server {
  pub(super) url: String,
  pub(super) requests: Arc<Mutex<Vec<Request>>>,
  pub(super) closed: Arc<AtomicUsize>,
  pub(super) hanging: Arc<AtomicUsize>,
  pub(super) task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
  fn drop(&mut self) {
    self.task.abort();
  }
}
impl Server {
  pub(super) async fn start(handler: impl Fn(Request) -> Reply + Send + Sync + 'static) -> Self {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let log = requests.clone();
    let closed = Arc::new(AtomicUsize::new(0));
    let closure = closed.clone();
    let hanging = Arc::new(AtomicUsize::new(0));
    let active_hangs = hanging.clone();
    let handler = Arc::new(handler);
    let task = tokio::spawn(async move {
      let mut tasks = JoinSet::new();
      loop {
        tokio::select! {
          accepted = listener.accept() => {
            let (mut stream, _) = accepted.unwrap();
            let handler = handler.clone();
            let log = log.clone();
            let closure = closure.clone();
            let active_hangs = active_hangs.clone();
            tasks.spawn(async move {
              let mut bytes = Vec::new();
              while !bytes.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                if stream.read_exact(&mut byte).await.is_err() { return; }
                bytes.push(byte[0]);
                assert!(bytes.len() <= 8192);
              }
              let headers = String::from_utf8(bytes).unwrap();
              let mut lines = headers.lines();
              let path = lines.next().unwrap().split_whitespace().nth(1).unwrap().to_string();
              let mut request = Request { path, range: None, identity: false, referer: None };
              for line in lines {
                if let Some((key, value)) = line.split_once(':') {
                  if key.eq_ignore_ascii_case("range") { request.range = Some(value.trim().into()); }
                  if key.eq_ignore_ascii_case("referer") { request.referer = Some(value.trim().into()); }
                  if key.eq_ignore_ascii_case("accept-encoding") { request.identity = value.trim() == "identity"; }
                }
              }
              log.lock().unwrap().push(request.clone());
              let reply = handler(request);
              let mut headers = format!("HTTP/1.1 {} Test\r\nConnection: close\r\n", reply.status);
              if let Some(length) = reply.length { headers.push_str(&format!("Content-Length: {length}\r\n")); }
              if let Some(range) = reply.range { headers.push_str(&format!("Content-Range: {range}\r\n")); }
              if let Some(encoding) = reply.encoding { headers.push_str(&format!("Content-Encoding: {encoding}\r\n")); }
              if let Some(location) = reply.location { headers.push_str(&format!("Location: {location}\r\n")); }
              headers.push_str("\r\n");
              if stream.write_all(headers.as_bytes()).await.is_err() { return; }
              if reply.hang {
                active_hangs.fetch_add(1, Ordering::SeqCst);
                let mut buffer = [0; 128];
                while let Ok(n) = stream.read(&mut buffer).await { if n == 0 { break; } }
                closure.fetch_add(1, Ordering::SeqCst);
              } else {
                let _ = stream.write_all(&reply.body).await;
              }
            });
          },
          result = tasks.join_next(), if !tasks.is_empty() => { result.unwrap().unwrap(); },
        }
      }
    });
    Self {
      url,
      requests,
      closed,
      hanging,
      task,
    }
  }
}

pub(super) fn fixture(bytes: &[u8]) -> Seed {
  let leaves: Vec<_> = bytes
    .chunks(DEFAULT_BLOCK_SIZE as usize)
    .map(merkle::leaf_hash)
    .collect();
  let mut seed = Seed::new(Info::new(bytes.len() as u64, merkle::root(&leaves)));
  seed.leaf_hashes = Some(leaves);
  seed
}
pub(super) fn range_reply(request: &Request, bytes: &[u8]) -> Reply {
  let range = request
    .range
    .as_ref()
    .unwrap()
    .strip_prefix("bytes=")
    .unwrap();
  let (start, end) = range.split_once('-').unwrap();
  let start: usize = start.parse().unwrap();
  let end: usize = end.parse().unwrap();
  let mut reply = Reply::body(bytes[start..=end].to_vec());
  reply.status = 206;
  reply.range = Some(format!("bytes {start}-{end}/{}", bytes.len()));
  reply
}
pub(super) async fn client(root: &std::path::Path, concurrency: usize) -> SdkClient {
  crate::client::builder()
    .track_url("http://127.0.0.1:1")
    .identity_file(root.join("identity"))
    .storage_dir(root.join("storage"))
    .max_download_peers(concurrency)
    .build()
    .await
    .unwrap()
}
