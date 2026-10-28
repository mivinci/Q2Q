mod cert;
mod limits;
mod registry;
mod server;
mod signalling;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use clap::Parser;
use limits::Rates;
use q2q_punch::binding::{Bindings, now};
use q2q_punch::signal::Connections;
use signalling::Sinks;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

/// Q2Q Punch 服务：Binding 的接入点。
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
  /// 配置文件路径（TOML）
  #[arg(default_value = "punch.toml")]
  config: PathBuf,
  /// Initialize persistent identity, print its public key and exit without listening.
  #[arg(long)]
  init_identity: bool,
  /// Import an existing Ed25519 PKCS#8 PEM private key during initialization.
  #[arg(long, requires = "init_identity")]
  identity_key: Option<PathBuf>,
  /// 日志级别
  #[arg(long, env = "RUST_LOG", default_value = "info")]
  log: String,
}

#[derive(Debug, serde::Deserialize)]
struct Config {
  bind: SocketAddr,
  punch_id: String,
  /// Enable in-process WebSocket TLS; leave disabled behind a TLS reverse proxy.
  #[serde(default)]
  websocket_tls: bool,
  #[serde(default = "default_ttl")]
  ttl_secs: u64,
  #[serde(default = "default_sweep")]
  sweep_secs: u64,
  #[serde(default = "default_max")]
  max_request_bytes: usize,
  #[serde(default)]
  limits: LimitConfig,
  /// A real deployment serves a real certificate. Omit it and a self-signed one
  /// is minted at startup — fine for tests and nothing else.
  identity: Option<IdentityConfig>,
  registry: Option<registry::Config>,
}

/// Buckets live in memory, so a multi-instance deployment divides the quota
/// by the number of instances rather than sharing a bucket.
#[derive(Debug, serde::Deserialize)]
#[serde(default)]
struct LimitConfig {
  join_per_minute: u32,
  heartbeat_per_minute: u32,
  connect_per_minute: u32,
  signal_per_minute: u32,
  /// How many Punch instances share these limits. Bucket size is divided by it.
  cluster_size: u32,
}

impl Default for LimitConfig {
  fn default() -> Self {
    let Rates {
      join,
      heartbeat,
      connect,
      signal,
    } = Rates::default();
    LimitConfig {
      join_per_minute: join,
      heartbeat_per_minute: heartbeat,
      connect_per_minute: connect,
      signal_per_minute: signal,
      cluster_size: 1,
    }
  }
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
    .with_writer(std::io::stderr)
    .init();

  let config = Config::load(&args.config)?;
  if args.init_identity {
    let paths = config
      .identity
      .as_ref()
      .ok_or("--init-identity requires [identity] certificate and key paths")?;
    let identity = match args.identity_key {
      Some(source) => {
        cert::initialize_from_key(&paths.cert, &paths.key, &config.punch_id, &source)?
      }
      None => cert::load_or_create(&paths.cert, &paths.key, &config.punch_id)?,
    };
    println!("{}", B64.encode(identity.public_key()));
    return Ok(());
  }
  let registry_token_configured =
    config.registry.is_some() || std::env::var_os("Q2Q_REGISTRY_TOKEN").is_some();
  if registry_token_configured && config.identity.is_none() {
    return Err(
      "Registry credentials require persistent [identity]; initialize it before starting".into(),
    );
  }

  let identity = match &config.identity {
    // kept on disk, so the key registered at Track stays valid across restarts
    Some(id) if registry_token_configured => cert::load_existing(&id.cert, &id.key)?,
    Some(id) => cert::load_or_create(&id.cert, &id.key, &config.punch_id)?,
    None => {
      tracing::warn!(
        "no [identity] configured — this key is thrown away on restart, \
         and every client that registered the old one will reject this server"
      );
      cert::PunchIdentity::generate(&config.punch_id)?
    }
  };

  let registry_reporter = match &config.registry {
    Some(registry) => {
      let token = std::env::var("Q2Q_REGISTRY_TOKEN")
        .map(Some)
        .or_else(|error| {
          if matches!(error, std::env::VarError::NotPresent) {
            Ok(None)
          } else {
            Err("Q2Q_REGISTRY_TOKEN must be valid UTF-8")
          }
        })?;
      Some(registry::Reporter::prepare(
        registry,
        &config.punch_id,
        identity.public_key(),
        token,
      )?)
    }
    None if registry_token_configured => {
      return Err("Q2Q_REGISTRY_TOKEN requires a [registry] section".into());
    }
    None => None,
  };

  // clients check this server's certificate against this key, so it is what
  // has to be registered at Track as `punch_public_key`
  tracing::info!(
    punch = %config.punch_id,
    punch_public_key = %B64.encode(identity.public_key()),
    "server identity"
  );

  let websocket_listener = TcpListener::bind(config.bind).await?;
  let endpoint = server::endpoint(&identity, config.bind)?;
  tracing::info!(bind = %config.bind, "QUIC and WebSocket listeners ready");

  let state = Arc::new(server::State {
    bindings: Bindings::new(config.ttl_secs),
    connections: Connections::new(config.ttl_secs),
    sinks: Arc::new(Sinks::default()),
    limits: limits::limiter(
      Rates {
        join: config.limits.join_per_minute,
        heartbeat: config.limits.heartbeat_per_minute,
        connect: config.limits.connect_per_minute,
        signal: config.limits.signal_per_minute,
      },
      config.limits.cluster_size,
    ),
    punch_id: config.punch_id.clone(),
    ttl: config.ttl_secs,
    max_request_bytes: config.max_request_bytes,
    quic_slots: Arc::new(tokio::sync::Semaphore::new(server::CONNECTION_LIMIT)),
    websocket_slots: Arc::new(tokio::sync::Semaphore::new(server::CONNECTION_LIMIT)),
  });

  let sweep_state = state.clone();
  let sweep_every = config.sweep_secs.max(1);
  let sweep_task = tokio::spawn(async move {
    loop {
      tokio::time::sleep(std::time::Duration::from_secs(sweep_every)).await;
      let dropped = sweep_state.bindings.sweep(now());
      if dropped > 0 {
        tracing::info!(dropped, "swept expired bindings");
      }
      let closed = sweep_state.connections.sweep(now());
      if closed > 0 {
        tracing::info!(closed, "swept finished connections");
      }
      sweep_state
        .sinks
        .sweep(&sweep_state.bindings, &sweep_state.connections, now());
      // buckets idle for ten minutes, so a scan cannot keep them forever
      let dropped = sweep_state.limits.sweep(limits::now_ms(), 600_000);
      if dropped > 0 {
        tracing::debug!(dropped, "swept idle rate limit buckets");
      }
      if dropped > 0 {
        tracing::info!(dropped, "swept expired bindings");
      }
    }
  });

  let acceptor = if config.websocket_tls {
    Some(TlsAcceptor::from(Arc::new(identity.tls_server_config()?)))
  } else {
    None
  };
  tracing::info!(tls = config.websocket_tls, "WebSocket listener configured");
  let (registry_stop, stop) = tokio::sync::watch::channel(false);
  let registry_task =
    registry_reporter.map(|reporter| tokio::spawn(reporter.run(state.clone(), stop)));
  let mut quic_task = tokio::spawn(server::serve(endpoint.clone(), state.clone()));
  let result = tokio::select! {
    result = server::serve_websocket(websocket_listener, acceptor, state) => result,
    result = shutdown_signal() => result,
    _ = &mut quic_task => Err(std::io::Error::other("QUIC listener stopped unexpectedly")),
  };
  let _ = registry_stop.send(true);
  if let Some(task) = registry_task
    && task.await.is_err()
  {
    tracing::error!("Registry reporting task exited unexpectedly");
  }
  sweep_task.abort();
  endpoint.close(0u32.into(), b"server shutdown");
  quic_task.abort();
  result?;
  Ok(())
}

async fn shutdown_signal() -> std::io::Result<()> {
  #[cfg(unix)]
  {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! { result = tokio::signal::ctrl_c() => result, _ = terminate.recv() => Ok(()) }
  }
  #[cfg(not(unix))]
  tokio::signal::ctrl_c().await
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn websocket_tls_is_opt_in() {
    let base = "bind = '127.0.0.1:4433'\npunch_id = 'punch-1'\n";
    let default: Config = toml::from_str(base).unwrap();
    assert!(!default.websocket_tls);
    let enabled: Config = toml::from_str(&format!("{base}websocket_tls = true\n")).unwrap();
    assert!(enabled.websocket_tls);
  }

  #[tokio::test]
  async fn tcp_and_udp_listeners_share_the_configured_address() {
    let identity = cert::PunchIdentity::generate("punch-1").unwrap();
    let requested = "127.0.0.1:0".parse().unwrap();
    let quic = server::endpoint(&identity, requested).unwrap();
    let bind = quic.local_addr().unwrap();
    let websocket = TcpListener::bind(bind).await.unwrap();

    assert_eq!(websocket.local_addr().unwrap(), bind);
    assert_eq!(quic.local_addr().unwrap(), bind);
  }
}
