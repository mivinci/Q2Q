//! Terminal rendering consumes SDK snapshots; it never owns transfer state.
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use q2q_core::client::{
  DownloadPeerProgress, PublishPhase, PublishProgress, TransferProgress, TransferState,
  UploadProgress,
};
use std::{
  collections::BTreeMap,
  io::{self, IsTerminal, Write},
  sync::OnceLock,
  time::{Duration, Instant},
};

// All live regions share one renderer so diagnostic writes can suspend drawing.
fn render_group() -> MultiProgress {
  static GROUP: OnceLock<MultiProgress> = OnceLock::new();
  GROUP
    .get_or_init(|| MultiProgress::with_draw_target(ProgressDrawTarget::stderr()))
    .clone()
}

pub(crate) struct DiagnosticWriter;
impl Write for DiagnosticWriter {
  fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
    render_group().suspend(|| io::stderr().lock().write(bytes))
  }
  fn flush(&mut self) -> io::Result<()> {
    io::stderr().lock().flush()
  }
}

struct Rate {
  last: Instant,
  bytes: u64,
}

pub(crate) struct MetadataDisplay(Option<ProgressBar>);
impl MetadataDisplay {
  pub(crate) fn new(quiet: bool) -> Self {
    if quiet {
      return Self(None);
    }
    if io::stderr().is_terminal() && std::env::var("TERM").as_deref() != Ok("dumb") {
      let bar = render_group().add(ProgressBar::new_spinner());
      bar.set_style(ProgressStyle::with_template("{spinner} {msg}").expect("metadata template"));
      bar.set_message("Finding peers and fetching metadata...");
      bar.enable_steady_tick(Duration::from_millis(100));
      Self(Some(bar))
    } else {
      eprintln!("Finding peers and fetching metadata...");
      Self(None)
    }
  }
}
impl Drop for MetadataDisplay {
  fn drop(&mut self) {
    if let Some(bar) = &self.0 {
      bar.finish_and_clear();
      render_group().remove(bar);
    }
  }
}
impl Rate {
  fn new(bytes: u64) -> Self {
    Self {
      last: Instant::now(),
      bytes,
    }
  }
  fn sample(&mut self, bytes: u64) -> f64 {
    let now = Instant::now();
    let rate = bytes.saturating_sub(self.bytes) as f64
      / now.duration_since(self.last).as_secs_f64().max(0.001);
    self.last = now;
    self.bytes = bytes;
    rate
  }
}
struct PeerRow {
  bar: ProgressBar,
  rate: Rate,
}
struct Terminal {
  group: MultiProgress,
  summary: ProgressBar,
  peers: BTreeMap<String, PeerRow>,
}
pub(crate) struct Progress {
  rate: Rate,
  started: Instant,
  terminal: Option<Terminal>,
  download_style: bool,
  plain_rates: BTreeMap<u64, Rate>,
}

pub(crate) struct PreparationDisplay {
  file: String,
  started: Instant,
  last_plain: Instant,
  rate: Rate,
  latest: PublishProgress,
  last_rendered: Option<PublishProgress>,
  terminal: Option<ProgressBar>,
  quiet: bool,
  finished: bool,
}

impl PreparationDisplay {
  pub(crate) fn new(file: &std::path::Path, quiet: bool) -> Self {
    let interactive =
      !quiet && io::stderr().is_terminal() && std::env::var("TERM").as_deref() != Ok("dumb");
    let terminal = interactive.then(|| {
      let bar = render_group().add(ProgressBar::new_spinner());
      bar.set_style(preparation_style());
      bar.enable_steady_tick(Duration::from_millis(100));
      bar
    });
    Self {
      file: file.display().to_string(),
      started: Instant::now(),
      last_plain: Instant::now() - Duration::from_secs(1),
      rate: Rate::new(0),
      latest: PublishProgress::default(),
      last_rendered: None,
      terminal,
      quiet,
      finished: false,
    }
  }

  pub(crate) fn update(&mut self, progress: PublishProgress) {
    if self.quiet {
      self.latest = progress;
      return;
    }
    self.latest = progress;
    let rate = self.rate.sample(progress.bytes_read);
    let elapsed = duration(self.started.elapsed());
    let message = match progress.phase {
      PublishPhase::Preparing => {
        let percent = if progress.total_bytes == 0 {
          0.0
        } else {
          (progress.bytes_read as f64 / progress.total_bytes as f64 * 100.0).min(100.0)
        };
        let remaining = progress.total_bytes.saturating_sub(progress.bytes_read);
        let eta = if remaining == 0 {
          "0s".into()
        } else if rate > 0.0 {
          duration(Duration::from_secs_f64(
            (remaining as f64 / rate).ceil().min(u32::MAX as f64),
          ))
        } else {
          "--".into()
        };
        format!(
          "Preparing {}: {percent:.1}% | {}/{} | {}/s | ETA {eta} | elapsed {elapsed}",
          self.file,
          bytes(progress.bytes_read as f64),
          bytes(progress.total_bytes as f64),
          bytes(rate),
        )
      }
      PublishPhase::Announcing => format!(
        "Announcing {} on Track | prepared {} | elapsed {elapsed}",
        self.file,
        bytes(progress.total_bytes as f64),
      ),
    };
    if let Some(bar) = &self.terminal {
      bar.set_length(progress.total_bytes.max(1));
      bar.set_position(progress.bytes_read.min(progress.total_bytes));
      bar.set_message(message);
    } else if self.last_rendered != Some(progress)
      && (self.last_rendered.is_none()
        || self.latest.phase != self.last_rendered.expect("checked above").phase
        || self.last_plain.elapsed() >= Duration::from_secs(1)
        || progress.bytes_read == progress.total_bytes)
    {
      eprintln!("{message}");
      self.last_plain = Instant::now();
      self.last_rendered = Some(progress);
    }
  }

  pub(crate) fn finish(&mut self) {
    if self.finished {
      return;
    }
    if !self.quiet {
      if self.terminal.is_none() && self.last_rendered != Some(self.latest) {
        self.last_plain = Instant::now() - Duration::from_secs(1);
        self.update(self.latest);
      }
      if let Some(bar) = &self.terminal {
        bar.finish_and_clear();
        render_group().remove(bar);
      }
    }
    self.finished = true;
  }
}

impl Drop for PreparationDisplay {
  fn drop(&mut self) {
    self.finish();
  }
}

impl Progress {
  pub(crate) fn new(bytes: u64, quiet: bool) -> Self {
    let interactive =
      !quiet && io::stderr().is_terminal() && std::env::var("TERM").as_deref() != Ok("dumb");
    let terminal = interactive.then(|| {
      let group = render_group();
      // Add before drawing: MultiProgress must own every live line.
      let summary = group.add(ProgressBar::new_spinner());
      summary.set_style(upload_style());
      summary.enable_steady_tick(Duration::from_millis(100));
      Terminal {
        group,
        summary,
        peers: BTreeMap::new(),
      }
    });
    Self {
      rate: Rate::new(bytes),
      started: Instant::now(),
      terminal,
      download_style: false,
      plain_rates: BTreeMap::new(),
    }
  }
  pub(crate) fn download(&mut self, p: TransferProgress, peers: &[DownloadPeerProgress]) {
    let rate = self.rate.sample(p.verified_bytes);
    let remaining = p.total_bytes.saturating_sub(p.verified_bytes);
    let eta = if remaining == 0 {
      "0s".into()
    } else if rate > 0.0 {
      duration(Duration::from_secs_f64(
        (remaining as f64 / rate).ceil().min(u32::MAX as f64),
      ))
    } else {
      "--".into()
    };
    let message = format!(
      "{:?} | {}/{} | {}/s verified | ETA {} | peers {} | HTTP {} | elapsed {}",
      p.state,
      bytes(p.verified_bytes as f64),
      bytes(p.total_bytes as f64),
      bytes(rate),
      eta,
      p.active_peers,
      p.active_cdn_requests,
      duration(self.started.elapsed())
    );
    if let Some(terminal) = &mut self.terminal {
      if !self.download_style {
        terminal.summary.disable_steady_tick();
        terminal.summary.set_style(download_style());
        self.download_style = true;
      }
      terminal.summary.set_length(p.total_bytes.max(1));
      // Empty files become complete only when the SDK reports Completed.
      let position = if p.total_bytes == 0 && p.state == TransferState::Completed {
        1
      } else {
        p.verified_bytes.min(p.total_bytes)
      };
      terminal.summary.set_position(position);
      terminal.summary.set_message(message);
      terminal.peers.retain(|id, row| {
        if peers.iter().any(|peer| peer.attempt_id.to_string() == *id) {
          true
        } else {
          row.bar.finish_and_clear();
          terminal.group.remove(&row.bar);
          false
        }
      });
      for peer in peers {
        let row = terminal
          .peers
          .entry(peer.attempt_id.to_string())
          .or_insert_with(|| {
            let bar = terminal.group.add(ProgressBar::new_spinner());
            bar.set_style(peer_style());
            bar.set_prefix(format!("  peer {}", short(peer.peer_id.as_str())));
            PeerRow {
              bar,
              rate: Rate::new(peer.received_bytes),
            }
          });
        row.bar.set_message(format!(
          "{:?} | rx {} | {}/s | verified {} | attempt {}",
          peer.state,
          bytes(peer.received_bytes as f64),
          bytes(row.rate.sample(peer.received_bytes)),
          bytes(peer.verified_bytes as f64),
          peer.attempt_id
        ));
      }
    } else {
      let percent = if p.total_bytes == 0 {
        if p.state == TransferState::Completed {
          100.0
        } else {
          0.0
        }
      } else {
        (p.verified_bytes as f64 / p.total_bytes as f64 * 100.0).min(100.0)
      };
      eprintln!("download {percent:.1}%: {message}");
      self
        .plain_rates
        .retain(|id, _| peers.iter().any(|peer| peer.attempt_id == *id));
      for peer in peers {
        let rate = self
          .plain_rates
          .entry(peer.attempt_id)
          .or_insert_with(|| Rate::new(peer.received_bytes))
          .sample(peer.received_bytes);
        eprintln!(
          "  peer {}: {:?}; received {}; {}/s; verified {}; attempt {}",
          peer.peer_id,
          peer.state,
          bytes(peer.received_bytes as f64),
          bytes(rate),
          bytes(peer.verified_bytes as f64),
          peer.attempt_id
        );
      }
    }
  }
  pub(crate) fn upload(&mut self, p: UploadProgress) {
    let rate = self.rate.sample(p.sent_bytes);
    let message = format!(
      "{:?} | sent {} | {}/s | peers {} active / {} handshake / {} idle | sessions {} done / {} failed / {} cancelled",
      p.state,
      bytes(p.sent_bytes as f64),
      bytes(rate),
      p.active_connections,
      p.handshaking_connections,
      p.idle_connections,
      p.completed_connections,
      p.failed_connections,
      p.cancelled_connections
    );
    if let Some(terminal) = &mut self.terminal {
      terminal.summary.set_message(message);
      terminal.peers.retain(|id, row| {
        if p
          .connections
          .iter()
          .any(|connection| &connection.connection_id == id)
        {
          true
        } else {
          row.bar.finish_and_clear();
          terminal.group.remove(&row.bar);
          false
        }
      });
      for connection in p.connections {
        let row = terminal
          .peers
          .entry(connection.connection_id.clone())
          .or_insert_with(|| {
            let bar = terminal.group.add(ProgressBar::new_spinner());
            bar.set_style(peer_style());
            bar.set_prefix(format!("  peer {}", short(connection.peer_id.as_str())));
            PeerRow {
              bar,
              rate: Rate::new(connection.sent_bytes),
            }
          });
        let rate = row.rate.sample(connection.sent_bytes);
        row.bar.set_message(format!(
          "{:?} | sent {} | {}/s | retry {} | conn {}",
          connection.state,
          bytes(connection.sent_bytes as f64),
          bytes(rate),
          bytes(connection.retransmitted_bytes as f64),
          short(&connection.connection_id)
        ));
      }
    } else {
      eprintln!("upload: {message}");
      for connection in p.connections {
        eprintln!(
          "  peer {}: {:?}; sent {}; retransmitted {}",
          connection.peer_id,
          connection.state,
          bytes(connection.sent_bytes as f64),
          bytes(connection.retransmitted_bytes as f64)
        );
      }
    }
  }
}
impl Drop for Progress {
  fn drop(&mut self) {
    if let Some(terminal) = &self.terminal {
      // Remove the live region before callers print a result/error or start
      // awaiting shutdown. Never leave a spinner alive during cleanup.
      terminal.summary.finish_and_clear();
      for row in terminal.peers.values() {
        row.bar.finish_and_clear();
      }
      terminal.group.remove(&terminal.summary);
      for row in terminal.peers.values() {
        terminal.group.remove(&row.bar);
      }
    }
  }
}
fn upload_style() -> ProgressStyle {
  ProgressStyle::with_template("{spinner:.green} Upload {wide_msg}")
    .expect("static upload template")
}
fn preparation_style() -> ProgressStyle {
  ProgressStyle::with_template("{spinner:.green} {wide_bar:.cyan/blue} {wide_msg}")
    .expect("static preparation template")
    .progress_chars("=>-")
}
fn download_style() -> ProgressStyle {
  ProgressStyle::with_template("Download [{wide_bar:.cyan/blue}] {percent:>3}%\n  {msg}")
    .expect("static download template")
    .progress_chars("=>-")
}
fn peer_style() -> ProgressStyle {
  ProgressStyle::with_template("{prefix} {wide_msg}").expect("static peer template")
}
fn short(value: &str) -> String {
  value.chars().take(12).collect()
}
fn bytes(value: f64) -> String {
  let mut value = value;
  let units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let mut unit = 0;
  while value >= 1024.0 && unit < units.len() - 1 {
    value /= 1024.0;
    unit += 1;
  }
  if unit == 0 {
    format!("{value:.0} {}", units[unit])
  } else {
    format!("{value:.1} {}", units[unit])
  }
}
fn duration(value: Duration) -> String {
  let seconds = value.as_secs();
  if seconds >= 3600 {
    format!("{}h{:02}m", seconds / 3600, seconds / 60 % 60)
  } else if seconds >= 60 {
    format!("{}m{:02}s", seconds / 60, seconds % 60)
  } else {
    format!("{seconds}s")
  }
}
