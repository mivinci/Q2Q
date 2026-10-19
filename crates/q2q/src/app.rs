use crate::args::{Args, Command};
use crate::progress::Progress;
use crate::shutdown::{Stop, initialize};
use crate::{Outcome, Result};
use q2q_core::client::{Client, Publication};
use q2q_primitives::seed::Seed;
use std::{
  io::{self, Write},
  path::{Path, PathBuf},
  time::Duration,
};
use tokio::io::AsyncReadExt;

pub(crate) async fn run(args: Args, mut stop: Stop) -> Result<Outcome> {
  let seed = match &args.command {
    Command::Download { seed, .. } => Some(read_seed(seed).await?),
    Command::Publish { .. } => None,
  };
  let track = args.track.as_deref()
    .or_else(|| seed.as_ref()?.track_list.first().map(String::as_str))
    .ok_or_else(|| io::Error::other("Track URL required: pass --track or Q2Q_TRACK_URL (downloads may use the seed's first Track)"))?;
  let mut builder = q2q_core::client::builder()
    .track_url(track)
    .storage_dir(&args.data_dir)
    .identity_file(
      args
        .identity
        .clone()
        .unwrap_or_else(|| args.data_dir.join("identity")),
    )
    .max_download_peers(args.download_peers.into())
    .max_upload_peers(args.upload_peers.into());
  if let Some(rate) = args.upload_rate {
    builder = builder.upload_rate_limit(rate);
  }
  let (client, interrupted) = initialize(async { Ok(builder.build().await?) }, &mut stop).await?;
  if interrupted {
    return Ok(Outcome::Interrupted);
  }
  if !args.quiet {
    eprintln!("Local peer: {}", client.peer_id());
  }
  match &args.command {
    Command::Publish { file, seed } => publish(&client, file, seed, args.quiet, &mut stop).await,
    Command::Download { output, resume, .. } => {
      download(
        &client,
        seed.as_ref().expect("download seed loaded"),
        output,
        *resume,
        args.quiet,
        &mut stop,
      )
      .await
    }
  }
}

async fn read_seed(path: &Path) -> Result<Seed> {
  // Seed inputs are untrusted. Bound allocation even when the input is not a
  // regular file or grows between metadata and reading.
  const MAX_SEED_BYTES: u64 = 8 * 1024 * 1024;
  let file = tokio::fs::File::open(path).await?;
  let mut bytes = Vec::new();
  file
    .take(MAX_SEED_BYTES + 1)
    .read_to_end(&mut bytes)
    .await?;
  if bytes.len() as u64 > MAX_SEED_BYTES {
    return Err(io::Error::other("seed exceeds the 8 MiB CLI input limit").into());
  }
  Ok(Seed::parse(&bytes)?)
}

async fn publish(
  client: &Client,
  file: &Path,
  seed_path: &Path,
  quiet: bool,
  stop: &mut Stop,
) -> Result<Outcome> {
  // Reserve a temporary file beside the destination before announcing. The
  // final persist is atomic and refuses even a concurrently created target.
  let target = seed_path.to_path_buf();
  let staged = tokio::task::spawn_blocking(move || -> io::Result<_> {
    match std::fs::symlink_metadata(&target) {
      Ok(_) => {
        return Err(io::Error::new(
          io::ErrorKind::AlreadyExists,
          "seed output already exists",
        ));
      }
      Err(error) if error.kind() == io::ErrorKind::NotFound => {}
      Err(error) => return Err(error),
    }
    let parent = target
      .parent()
      .filter(|p| !p.as_os_str().is_empty())
      .unwrap_or(Path::new("."));
    tempfile::NamedTempFile::new_in(parent)
  })
  .await??;
  if !quiet {
    eprintln!("Preparing and announcing {}...", file.display());
  }
  let (publication, interrupted) =
    initialize(async { Ok(client.publish_file(file).await?) }, stop).await?;
  if interrupted {
    publication.close().await?;
    return Ok(Outcome::Interrupted);
  }
  let result = serve(&publication, staged, seed_path.to_path_buf(), quiet, stop).await;
  // Always await SDK cleanup, including output/export errors. Drop alone only
  // requests asynchronous withdrawal, which process exit could interrupt.
  let closed = publication.close().await;
  match result {
    Err(error) => {
      if let Err(cleanup) = closed {
        eprintln!("q2q: publication cleanup: {cleanup}");
      }
      Err(error)
    }
    Ok(outcome) => {
      closed?;
      Ok(outcome)
    }
  }
}

async fn serve(
  publication: &Publication,
  mut staged: tempfile::NamedTempFile,
  target: PathBuf,
  quiet: bool,
  stop: &mut Stop,
) -> Result<Outcome> {
  let encoded = publication.seed().encode();
  let output = target.clone();
  tokio::task::spawn_blocking(move || -> io::Result<()> {
    staged.write_all(&encoded)?;
    staged.as_file().sync_all()?;
    staged.persist_noclobber(output).map_err(|e| e.error)?;
    Ok(())
  })
  .await??;
  writeln!(io::stdout().lock(), "{}", target.display())?;
  if !quiet {
    eprintln!("Published; keep this process running. Ctrl-C stops sharing.");
  }
  let mut progress = Progress::new(publication.progress().sent_bytes, quiet);
  let mut tick = ticker();
  loop {
    tokio::select! {
      signal = &mut *stop => { signal?; return Ok(Outcome::Interrupted); },
      _ = tick.tick() => {
        if !quiet { progress.upload(publication.progress()); }
        if publication.is_finished() {
          // Joining in the caller preserves the SDK's actual failure.
          return Ok(Outcome::Complete);
        }
      }
    }
  }
}

async fn download(
  client: &Client,
  seed: &Seed,
  output: &Path,
  resume: bool,
  quiet: bool,
  stop: &mut Stop,
) -> Result<Outcome> {
  let (transfer, interrupted) = initialize(
    async {
      Ok(if resume {
        client.resume_download(seed, output).await?
      } else {
        client.download(seed, output).await?
      })
    },
    stop,
  )
  .await?;
  if interrupted {
    transfer.cancel();
    return Ok(Outcome::Interrupted);
  }
  let updates = transfer.subscribe_progress();
  let peer_updates = transfer.subscribe_peer_progress();
  let mut progress = Progress::new(transfer.progress().verified_bytes, quiet);
  let mut tick = ticker();
  let path = {
    // The pinned wait future owns Transfer. Leaving this scope on a signal
    // drops it, cancelling network tasks while preserving committed receipts.
    let wait = transfer.wait();
    tokio::pin!(wait);
    loop {
      tokio::select! {
        result = &mut wait => break Some(result?),
        signal = &mut *stop => { signal?; break None; },
        _ = tick.tick() => {
          if !quiet {
            let total = *updates.borrow();
            let peers = peer_updates.borrow().clone();
            progress.download(total, &peers);
          }
        }
      }
    }
  };
  if !quiet {
    let total = *updates.borrow();
    let peers = peer_updates.borrow().clone();
    progress.download(total, &peers);
  }
  drop(progress);
  match path {
    Some(path) => {
      writeln!(io::stdout().lock(), "{}", path.display())?;
      Ok(Outcome::Complete)
    }
    None => {
      if !quiet {
        eprintln!("Download cancelled; keep the destination and .q2q.resume file to resume.");
      }
      Ok(Outcome::Interrupted)
    }
  }
}

fn ticker() -> tokio::time::Interval {
  let mut tick = tokio::time::interval(Duration::from_secs(1));
  tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
  tick
}
