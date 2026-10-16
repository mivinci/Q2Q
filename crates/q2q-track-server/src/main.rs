//! The Track server process.
//!
//! Everything protocol-shaped lives in `q2q-track`; this crate only wires it to
//! a socket, a config file and a sweep loop.

mod api;

use clap::Parser;
use q2q_track::state::{MemoryState, PunchAssignment, TrackState, UnixSeconds};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

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
}

#[derive(Debug, serde::Deserialize)]
struct PunchServer {
  punch_id: String,
  url: String,
  /// base64 of the Punch Server's public key; clients use it to authenticate
  /// the server they are told to talk to.
  public_key: String,
}

fn default_bind() -> SocketAddr {
  "127.0.0.1:8080".parse().unwrap()
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

impl Config {
  fn load(path: &PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(path)?;
    Ok(toml::from_str(&text)?)
  }

  fn punch_servers(&self) -> Vec<PunchAssignment> {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as B64;
    self
      .punch
      .iter()
      .map(|p| PunchAssignment {
        punch_id: p.punch_id.clone(),
        url: p.url.clone(),
        public_key: B64.decode(&p.public_key).unwrap_or_default(),
      })
      .collect()
  }
}

/// Q2Q Track 服务：资源索引与会话。
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
  /// 配置文件路径（TOML）
  #[arg(default_value = "track.toml")]
  config: PathBuf,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
  let args = Args::parse();
  let config = Config::load(&args.config)?;

  let state = Arc::new(MemoryState::new());
  let track_config = Arc::new(api::TrackConfig {
    punch_servers: config.punch_servers(),
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
      let removed = sweep_state.sweep(q2q_track::state::now());
      if removed > 0 {
        println!("sweep: removed {removed} expired online records");
      }
    }
  });

  let app = api::router(state, track_config);
  let listener = tokio::net::TcpListener::bind(config.bind).await?;
  println!("track listening on {bind}", bind = config.bind);
  axum::serve(listener, app).await?;
  Ok(())
}
