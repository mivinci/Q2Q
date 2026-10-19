mod app;
mod args;
mod progress;
mod shutdown;

use clap::Parser;
use std::{error::Error, process::ExitCode};

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

enum Outcome {
  Complete,
  Interrupted,
}

#[tokio::main]
async fn main() -> ExitCode {
  let args = args::Args::parse();
  let result = match shutdown::listen() {
    Ok(stop) => app::run(args, stop).await,
    Err(error) => Err(error.into()),
  };
  match result {
    Ok(Outcome::Complete) => ExitCode::SUCCESS,
    Ok(Outcome::Interrupted) => ExitCode::from(130),
    Err(error) => {
      eprintln!("q2q: {error}");
      let mut source = error.source();
      while let Some(cause) = source {
        eprintln!("  caused by: {cause}");
        source = cause.source();
      }
      ExitCode::FAILURE
    }
  }
}
