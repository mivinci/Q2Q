use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
  name = "q2q",
  version,
  about = "Publish and download verified files over Q2Q"
)]
pub(crate) struct Args {
  /// Track HTTP(S) URL; downloads otherwise use the seed's first Track.
  #[arg(long, global = true, env = "Q2Q_TRACK_URL")]
  pub track: Option<String>,
  /// Local state directory. Use a separate directory for each running client.
  #[arg(long, global = true, env = "Q2Q_DATA_DIR", default_value = ".q2q")]
  pub data_dir: PathBuf,
  /// Persistent private identity; defaults to <data-dir>/identity.
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
  /// Publish a file and keep serving until interrupted. Requires --track.
  Publish {
    file: PathBuf,
    /// Export the seed to this new file; never overwrite an existing file.
    #[arg(long)]
    seed: PathBuf,
  },
  /// Download using a seed. Existing destinations require explicit --resume.
  Download {
    seed: PathBuf,
    /// Exact destination filename, not a directory.
    #[arg(short, long)]
    output: PathBuf,
    /// Revalidate and resume an existing SDK destination and .q2q.resume file.
    #[arg(long)]
    resume: bool,
  },
}
