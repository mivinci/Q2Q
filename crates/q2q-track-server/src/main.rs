//! The Track server process.
//!
//! Everything protocol-shaped lives in `q2q-track`; this crate only wires it to
//! a socket, a config file and a sweep loop.

mod api;

use axum::Router;
use clap::Parser;
use q2q_core::identity::PeerId;
use q2q_track::redis::RedisState;
use q2q_track::state::{
  Candidate, InfoHash, MemoryState, OnlineRecord, PunchAssignment, StateError, TrackState,
  UnixSeconds,
};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Bind and serve. Pulled out of `main` so a test can drive the real path.
///
/// ConnectInfo has to be asked for explicitly: without it the rate limiter
/// finds no peer address and lets everything through. Unit tests cannot see
/// that, because they call the router directly and never go through here.
async fn serve(listener: tokio::net::TcpListener, app: Router) -> std::io::Result<()> {
  axum::serve(
    listener,
    app.into_make_service_with_connect_info::<SocketAddr>(),
  )
  .await
}

/// Config file shape. Punch servers are deployment facts, not protocol.
#[derive(Debug, serde::Deserialize)]
struct Config {
  #[serde(default = "default_bind")]
  bind: SocketAddr,
  #[serde(default)]
  punch: Vec<PunchServer>,
  #[serde(default = "default_turn_url")]
  turn_url: String,
  #[serde(default)]
  turn_secret: String,
  #[serde(default = "default_turn_ttl")]
  turn_ttl: UnixSeconds,
  #[serde(default = "default_max_ttl")]
  max_ttl: UnixSeconds,
  #[serde(default = "default_heartbeat")]
  heartbeat_interval: u32,
  #[serde(default = "default_sweep")]
  sweep_interval: u64,
  /// How often to check whether the Punch list file changed.
  #[serde(default = "default_reload")]
  reload_interval: u64,
  #[serde(default)]
  state: StateConfig,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(tag = "backend", rename_all = "lowercase")]
enum StateConfig {
  #[default]
  Memory,
  Redis {
    url: String,
  },
}

enum StateBackend {
  Memory(MemoryState),
  Redis(RedisState),
}

impl TrackState for StateBackend {
  async fn announce(
    &self,
    record: OnlineRecord,
    add: &[InfoHash],
    del: &[InfoHash],
    complete: &[InfoHash],
    ttl: UnixSeconds,
  ) -> Result<(), StateError> {
    match self {
      Self::Memory(s) => s.announce(record, add, del, complete, ttl).await,
      Self::Redis(s) => s.announce(record, add, del, complete, ttl).await,
    }
  }

  async fn online(
    &self,
    peer_id: &PeerId,
    now: UnixSeconds,
  ) -> Result<Option<OnlineRecord>, StateError> {
    match self {
      Self::Memory(s) => s.online(peer_id, now).await,
      Self::Redis(s) => s.online(peer_id, now).await,
    }
  }

  async fn advance_session(
    &self,
    peer_id: &PeerId,
    session_key: &[u8; 32],
    seq: u64,
    renew_until: Option<UnixSeconds>,
    now: UnixSeconds,
  ) -> Result<(), StateError> {
    match self {
      Self::Memory(s) => {
        s.advance_session(peer_id, session_key, seq, renew_until, now)
          .await
      }
      Self::Redis(s) => {
        s.advance_session(peer_id, session_key, seq, renew_until, now)
          .await
      }
    }
  }

  async fn query(
    &self,
    info_hash: &InfoHash,
    limit: usize,
    after: Option<&PeerId>,
    now: UnixSeconds,
  ) -> Result<Vec<Candidate>, StateError> {
    match self {
      Self::Memory(s) => s.query(info_hash, limit, after, now).await,
      Self::Redis(s) => s.query(info_hash, limit, after, now).await,
    }
  }

  async fn sweep(&self, now: UnixSeconds) -> Result<usize, StateError> {
    match self {
      Self::Memory(s) => s.sweep(now).await,
      Self::Redis(s) => s.sweep(now).await,
    }
  }
}

#[derive(Debug, serde::Deserialize)]
struct PunchServer {
  punch_id: String,
  url: String,
  #[serde(default)]
  websocket_url: Option<String>,
  /// base64 of the Punch Server's public key; clients use it to authenticate
  /// the server they are told to talk to.
  public_key: String,
}

fn default_bind() -> SocketAddr {
  // built rather than parsed: there is no string here that could fail
  SocketAddr::from(([127, 0, 0, 1], 8080))
}
fn default_turn_url() -> String {
  "turn:127.0.0.1:3478".into()
}
fn default_turn_ttl() -> UnixSeconds {
  3600
}
fn default_max_ttl() -> UnixSeconds {
  3600
}
fn default_heartbeat() -> u32 {
  30
}
fn default_sweep() -> u64 {
  60
}
fn default_reload() -> u64 {
  5
}

impl Config {
  fn load(path: &PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(path)?;
    Ok(toml::from_str(&text)?)
  }

  /// Rejects the whole config on a bad key: a wrong `public_key` would be
  /// handed to clients as the anchor for verifying that Punch Server, so
  /// silently falling back to an empty one is worse than refusing to start.
  fn punch_servers(&self) -> Result<Vec<PunchAssignment>, String> {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as B64;
    let mut out = Vec::new();
    for p in &self.punch {
      let key = B64
        .decode(&p.public_key)
        .map_err(|e| format!("punch '{}': public_key is not base64: {e}", p.punch_id))?;
      if key.len() != 32 {
        return Err(format!(
          "punch '{}': public_key must be 32 bytes, got {}",
          p.punch_id,
          key.len()
        ));
      }
      out.push(PunchAssignment {
        punch_id: p.punch_id.clone(),
        url: p.url.clone(),
        websocket_url: p.websocket_url.clone(),
        public_key: key,
      });
    }
    Ok(out)
  }
}

/// Q2Q Track 服务：文件索引与会话。
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
  /// 配置文件路径（TOML）
  #[arg(default_value = "track.toml")]
  config: PathBuf,

  /// 日志过滤，如 `info`、`q2q_track_server=debug`
  #[arg(long, default_value = "info", env = "RUST_LOG")]
  log: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
  let args = Args::parse();
  tracing_subscriber::fmt()
    .with_env_filter(
      tracing_subscriber::EnvFilter::try_from_default_env()
        .or_else(|_| tracing_subscriber::EnvFilter::try_new(&args.log))
        .unwrap_or_default(),
    )
    .init();

  let config = Config::load(&args.config)?;
  tracing::info!(?config.bind, "config loaded from {}", args.config.display());

  let state = Arc::new(match &config.state {
    StateConfig::Memory => StateBackend::Memory(MemoryState::new()),
    StateConfig::Redis { url } => StateBackend::Redis(RedisState::connect(url).await?),
  });
  let punch_list = api::PunchList::new(config.punch_servers()?);
  let track_config = Arc::new(api::TrackConfig {
    turn_url: config.turn_url.clone(),
    turn_secret: config.turn_secret.clone(),
    turn_ttl: config.turn_ttl,
    max_ttl: config.max_ttl,
    heartbeat_interval: config.heartbeat_interval,
  });

  // without this, expired records would accumulate forever
  let sweep_state = state.clone();
  let sweep_interval = config.sweep_interval;
  tokio::spawn(async move {
    loop {
      tokio::time::sleep(Duration::from_secs(sweep_interval)).await;
      match sweep_state.sweep(q2q_track::state::now()).await {
        Ok(removed) if removed > 0 => {
          tracing::info!(removed, "sweep dropped expired Track records");
        }
        Err(e) => tracing::error!(?e, "could not sweep Track state"),
        _ => {}
      }
    }
  });

  // the list is deployment state: an instance can move, so watch the file and
  // swap it in without a restart
  let watch = punch_list.clone();
  let path = args.config.clone();
  let every = Duration::from_secs(config.reload_interval.max(1));
  tokio::spawn(async move {
    let mut seen = modified(&path);
    loop {
      tokio::time::sleep(every).await;
      let now = modified(&path);
      if now == seen {
        continue;
      }
      seen = now;
      match Config::load(&path)
        .map_err(|e| e.to_string())
        .and_then(|c| c.punch_servers())
      {
        Ok(next) => {
          tracing::info!(count = next.len(), "reloaded the Punch list");
          watch.replace(next);
        }
        // keep serving the old list: a typo should not take the service down
        Err(e) => tracing::warn!("keeping the old Punch list: {e}"),
      }
    }
  });

  let app = api::router_with(state, track_config, punch_list, api::limiter());
  let listener = tokio::net::TcpListener::bind(config.bind).await?;
  tracing::info!(bind = %config.bind, "listening");
  serve(listener, app).await?;
  Ok(())
}

fn modified(path: &PathBuf) -> Option<std::time::SystemTime> {
  std::fs::metadata(path).ok()?.modified().ok()
}

#[cfg(test)]
mod tests {
  use super::*;
  use axum::extract::ConnectInfo;
  use axum::routing::get;
  use base64::Engine as _;
  use base64::engine::general_purpose::STANDARD as B64;
  use tokio::io::{AsyncReadExt, AsyncWriteExt};

  /// 限流靠 ConnectInfo 拿对端地址，而单测是直接调 router 的，永远走不到
  /// `serve` 这条路径。这个测试走一次真实连接，确认地址确实传到了 handler。
  #[tokio::test]
  async fn a_real_connection_sees_its_peer_address() {
    let app = Router::new().route(
      "/addr",
      get(|ConnectInfo(addr): ConnectInfo<SocketAddr>| async move { addr.ip().to_string() }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve(listener, app));

    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream
      .write_all(b"GET /addr HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
      .await
      .unwrap();
    let mut buf = String::new();
    stream.read_to_string(&mut buf).await.unwrap();

    assert!(buf.starts_with("HTTP/1.1 200"), "response was: {buf}");
    assert!(buf.ends_with("127.0.0.1"), "body was: {buf}");
  }

  fn config_with_key(key: &str) -> Config {
    let text = format!(
      "turn_secret = \"s\"\n\
       [[punch]]\n\
       punch_id = \"punch-1\"\n\
       url = \"https://punch-1.example.com\"\n\
       public_key = \"{key}\"\n"
    );
    toml::from_str(&text).expect("the fixture parses")
  }

  #[test]
  fn a_well_formed_punch_key_is_accepted() {
    let key = B64.encode([7u8; 32]);
    let list = config_with_key(&key).punch_servers().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].public_key, vec![7u8; 32]);
  }

  #[test]
  fn a_short_punch_key_refuses_to_start() {
    // it would be handed to clients as the anchor for verifying that server
    let key = B64.encode([7u8; 16]);
    let e = config_with_key(&key).punch_servers().unwrap_err();
    assert!(e.contains("punch-1"), "{e}");
    assert!(e.contains("32 bytes"), "{e}");
  }

  #[test]
  fn key_that_is_not_base64_refuses_to_start() {
    let e = config_with_key("not base64 !!!")
      .punch_servers()
      .unwrap_err();
    assert!(e.contains("punch-1"), "{e}");
  }
}
