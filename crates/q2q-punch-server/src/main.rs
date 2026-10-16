mod cert;
mod server;
mod signalling;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use clap::Parser;
use q2q_punch::binding::{Bindings, now};
use q2q_punch::signal::Connections;
use signalling::Sinks;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Q2Q Punch 服务：Binding 的接入点。
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
  /// 配置文件路径（TOML）
  #[arg(default_value = "punch.toml")]
  config: PathBuf,
  /// 日志级别
  #[arg(long, env = "RUST_LOG", default_value = "info")]
  log: String,
}

#[derive(Debug, serde::Deserialize)]
struct Config {
  bind: SocketAddr,
  punch_id: String,
  #[serde(default = "default_ttl")]
  ttl_secs: u64,
  #[serde(default = "default_sweep")]
  sweep_secs: u64,
  #[serde(default = "default_max")]
  max_request_bytes: usize,
  /// A real deployment serves a real certificate. Omit it and a self-signed one
  /// is minted at startup — fine for tests and nothing else.
  identity: Option<IdentityConfig>,
}

#[derive(Debug, serde::Deserialize)]
struct IdentityConfig {
  cert: PathBuf,
  key: PathBuf,
}

fn default_ttl() -> u64 {
  300
}
fn default_sweep() -> u64 {
  60
}
fn default_max() -> usize {
  64 * 1024
}

impl Config {
  fn load(path: &Path) -> Result<Config, Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(path)?;
    Ok(toml::from_str(&text)?)
  }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
  let args = Args::parse();
  tracing_subscriber::fmt()
    .with_env_filter(&args.log)
    .with_target(false)
    .init();

  let config = Config::load(&args.config)?;

  let identity = match &config.identity {
    Some(id) => {
      let cert = std::fs::read_to_string(&id.cert)?;
      let key = std::fs::read_to_string(&id.key)?;
      cert::PunchIdentity::from_pem(&cert, &key)?
    }
    None => {
      tracing::warn!("no certificate configured — minting a self-signed one");
      cert::PunchIdentity::generate(&config.punch_id)?
    }
  };

  // clients check this server's certificate against this key, so it is what
  // has to be registered at Track as `punch_public_key`
  tracing::info!(
    punch = %config.punch_id,
    punch_public_key = %B64.encode(identity.public_key()),
    "server identity"
  );

  let endpoint = server::endpoint(&identity, config.bind)?;
  tracing::info!(bind = %config.bind, "listening");

  let state = Arc::new(server::State {
    bindings: Bindings::new(config.ttl_secs),
    connections: Connections::new(config.ttl_secs),
    sinks: Arc::new(Sinks::default()),
    punch_id: config.punch_id.clone(),
    ttl: config.ttl_secs,
    max_request_bytes: config.max_request_bytes,
  });

  let sweep_state = state.clone();
  let sweep_every = config.sweep_secs.max(1);
  tokio::spawn(async move {
    loop {
      tokio::time::sleep(std::time::Duration::from_secs(sweep_every)).await;
      let dropped = sweep_state.bindings.sweep(now());
      let closed = sweep_state.connections.sweep(now());
      if closed > 0 {
        tracing::info!(closed, "swept finished connections");
      }
      if dropped > 0 {
        tracing::info!(dropped, "swept expired bindings");
      }
    }
  });

  server::serve(endpoint, state).await;
  Ok(())
}
