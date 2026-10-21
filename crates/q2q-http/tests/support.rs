#![allow(dead_code)]
use std::{
  sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
  },
  time::Duration,
};
use tokio::{
  io::{AsyncReadExt, AsyncWriteExt},
  task::{JoinHandle, JoinSet},
};
#[derive(Clone, Debug)]
pub struct Request {
  pub path: String,
  pub range: Option<String>,
  pub if_range: Option<String>,
  pub referer: Option<String>,
  pub identity: bool,
}
pub struct Reply {
  pub status: u16,
  pub body: Vec<u8>,
  pub headers: Vec<(String, String)>,
  pub length: Option<usize>,
  pub delay: Duration,
}
impl Reply {
  pub fn body(body: impl Into<Vec<u8>>) -> Self {
    let body = body.into();
    let length = Some(body.len());
    Self {
      status: 200,
      body,
      headers: vec![],
      length,
      delay: Duration::ZERO,
    }
  }
  pub fn header(mut self, k: &str, v: &str) -> Self {
    self.headers.push((k.into(), v.into()));
    self
  }
  pub fn range(r: &Request, body: &[u8]) -> Self {
    let Some(range) = r.range.as_deref() else {
      return Self::body(body);
    };
    let (a, b) = range
      .strip_prefix("bytes=")
      .unwrap()
      .split_once('-')
      .unwrap();
    let a: usize = a.parse().unwrap();
    let b: usize = b.parse().unwrap();
    let mut result =
      Self::body(&body[a..=b]).header("Content-Range", &format!("bytes {a}-{b}/{}", body.len()));
    result.status = 206;
    result
  }
}
pub struct Server {
  pub url: String,
  pub requests: Arc<Mutex<Vec<Request>>>,
  pub peak: Arc<AtomicUsize>,
  task: JoinHandle<()>,
}
impl Drop for Server {
  fn drop(&mut self) {
    self.task.abort();
  }
}
impl Server {
  pub async fn start(handler: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> Self {
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", socket.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(vec![]));
    let log = requests.clone();
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let max = peak.clone();
    let handler = Arc::new(handler);
    let task = tokio::spawn(async move {
      let mut tasks = JoinSet::new();
      loop {
        tokio::select! {
         accepted=socket.accept()=>{let(mut socket,_)=accepted.unwrap();let handler=handler.clone();let log=log.clone();let active=active.clone();let max=max.clone();tasks.spawn(async move {
          let mut input=vec![];loop {let mut c=[0];if socket.read_exact(&mut c).await.is_err(){return;}input.push(c[0]);if input.ends_with(b"\r\n\r\n"){break;}assert!(input.len()<16384);}
          let text=String::from_utf8(input).unwrap();let mut lines=text.lines();let path=lines.next().unwrap().split_whitespace().nth(1).unwrap().to_owned();
          let mut r=Request {path,range:None,if_range:None,referer:None,identity:false};for line in lines {if let Some((k,v))=line.split_once(':'){match k.to_ascii_lowercase().as_str(){"range"=>r.range=Some(v.trim().into()),"if-range"=>r.if_range=Some(v.trim().into()),"referer"=>r.referer=Some(v.trim().into()),"accept-encoding"=>r.identity=v.trim()=="identity",_=>{}}}}
          log.lock().unwrap().push(r.clone());let reply=handler(&r);
          let count=active.fetch_add(1,Ordering::SeqCst)+1;max.fetch_max(count,Ordering::SeqCst);
          let mut header=format!("HTTP/1.1 {} Test\r\nConnection: close\r\n",reply.status);if let Some(n)=reply.length {header.push_str(&format!("Content-Length: {n}\r\n"));}for(k,v)in reply.headers {header.push_str(&format!("{k}: {v}\r\n"));}header.push_str("\r\n");
          let result=socket.write_all(header.as_bytes()).await;if result.is_ok(){tokio::time::sleep(reply.delay).await;let _=socket.write_all(&reply.body).await;}active.fetch_sub(1,Ordering::SeqCst);
         });},
         result=tasks.join_next(),if !tasks.is_empty()=>{result.unwrap().unwrap();}
        }
      }
    });
    Self {
      url,
      requests,
      peak,
      task,
    }
  }
}
pub fn sha256(bytes: &[u8]) -> q2q_http::Checksum {
  use sha2::Digest;
  q2q_http::Checksum::Sha256(sha2::Sha256::digest(bytes).into())
}
pub fn request(
  path: impl Into<std::path::PathBuf>,
  url: &str,
  bytes: &[u8],
) -> q2q_http::DownloadRequest {
  q2q_http::DownloadRequest::new(path)
    .mirrors([url])
    .expected_size(bytes.len() as u64)
    .checksum(sha256(bytes))
}
pub async fn staged(task: &q2q_http::Task, n: u64) {
  let mut p = task.subscribe_progress();
  tokio::time::timeout(Duration::from_secs(5), async {
    loop {
      let snap = *p.borrow_and_update();
      if snap.staged_bytes >= n {
        return;
      }
      assert!(!snap.state.is_terminal(), "{:?}", snap);
      p.changed().await.unwrap();
    }
  })
  .await
  .unwrap();
}
