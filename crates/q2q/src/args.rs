use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
  name = "q2q",
  version,
  about = "Publish and download verified files over Q2Q",
  long_about = "Publish and download verified files over Q2Q.\n\nLocal identity and resume state use the platform user state directory by default. Set Q2Q_HOME to choose another root."
)]
pub(crate) struct Args {
  /// Preferred Track HTTP(S) URL; downloads try seed/official Tracks on setup or discovery failure.
  #[arg(long, global = true, env = "Q2Q_TRACK_URL")]
  pub track: Option<String>,
  /// Skip Track/CDN HTTPS certificate verification (unsafe; debugging only).
  #[arg(long, global = true)]
  pub insecure: bool,
  /// Persistent private identity; defaults to <Q2Q_HOME>/identity.
  #[arg(long, global = true, env = "Q2Q_IDENTITY_FILE")]
  pub identity: Option<PathBuf>,
  /// Maximum concurrent download peers or CDN requests (1-16).
  #[arg(long, global = true, default_value_t = 4, value_parser = clap::value_parser!(u8).range(1..=16))]
  pub download_peers: u8,
  /// Maximum admitted upload connections (1-16).
  #[arg(long, global = true, default_value_t = 4, value_parser = clap::value_parser!(u8).range(1..=16))]
  pub upload_peers: u8,
  /// Total encoded upload piece bytes/second, including retransmits; unset is unlimited.
  #[arg(long, global = true, value_parser = clap::value_parser!(u64).range(1..))]
  pub upload_rate: Option<u64>,
  /// Suppress periodic progress; errors and success results are still emitted.
  #[arg(long, global = true)]
  pub quiet: bool,
  #[command(subcommand)]
  pub command: Command,
}

#[derive(Subcommand)]
pub(crate) enum Command {
  /// Build a seed locally without announcing or serving the file.
  Seed {
    file: PathBuf,
    /// Seed output path; defaults to <FILE>.q2q and never overwrites a file.
    #[arg(short, long)]
    output: Option<PathBuf>,
  },
  /// Print a Q2Q magnet URL from a local seed without contacting the network.
  Magnet { seed: PathBuf },
  /// Publish a file or existing .q2q seed and keep serving until interrupted.
  Publish {
    #[arg(value_name = "INPUT")]
    file: PathBuf,
    /// Local source for a seed input; defaults to info.name beside that seed.
    #[arg(long = "file")]
    source: Option<PathBuf>,
    /// Export a new seed; raw files default to <FILE>.q2q, existing seeds are reused.
    #[arg(long)]
    seed: Option<PathBuf>,
  },
  /// Download a seed file or Q2Q magnet URL; checkpoints resume automatically.
  Download {
    #[arg(value_name = "INPUT")]
    seed: PathBuf,
    /// Destination filename; defaults to a safe name from the seed.
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Require checkpoint-based revalidation for an existing destination.
    #[arg(long)]
    resume: bool,
  },
}
