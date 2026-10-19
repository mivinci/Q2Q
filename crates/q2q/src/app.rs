use crate::args::{Args, Command};
use crate::progress::{PreparationDisplay, Progress};
use crate::shutdown::{Stop, initialize, initialize_with_progress};
use crate::{Outcome, Result};
use q2q_core::client::{Client, Publication};
use q2q_primitives::seed::Seed;
use std::{
  env,
  fs::{self, File, OpenOptions},
  io::{self, Write},
  path::{Path, PathBuf},
  time::Duration,
};
use tokio::io::AsyncReadExt;
use url::Url;

const DEFAULT_TRACK_URL: &str = "https://track.q2q.app";

pub(crate) async fn run(args: Args, mut stop: Stop) -> Result<Outcome> {
  let seed = match &args.command {
    Command::Download { seed, .. } => Some(read_seed(seed).await?),
    Command::Publish { .. } => None,
  };
  let publish_seed_path = match &args.command {
    Command::Publish { file, seed } => Some(match seed {
      Some(path) => path.clone(),
      None => default_seed_path(file)?,
    }),
    Command::Download { .. } => None,
  };
  let download_output = match &args.command {
    Command::Download {
      seed: path, output, ..
    } => Some(match output {
      Some(output) => output.clone(),
      None => default_download_path(path, seed.as_ref().expect("download seed loaded"))?,
    }),
    Command::Publish { .. } => None,
  };
  let mut resume_from_checkpoint = false;
  let mut offline_resume = None;
  let home = resolve_home()?;
  if matches!(&args.command, Command::Download { .. }) {
    let (transfer, interrupted) = initialize(
      async {
        Ok(
          q2q_core::client::resume_offline(
            seed.as_ref().expect("download seed loaded"),
            download_output
              .as_deref()
              .expect("download output path resolved"),
            home.clone(),
          )
          .await?,
        )
      },
      &mut stop,
    )
    .await?;
    if interrupted {
      if let q2q_core::client::OfflineResume::Complete(transfer) = transfer {
        transfer.cancel();
      }
      return Ok(Outcome::Interrupted);
    }
    match transfer {
      q2q_core::client::OfflineResume::NoCheckpoint => {}
      q2q_core::client::OfflineResume::NeedsNetwork(offline) => {
        resume_from_checkpoint = true;
        offline_resume = Some(offline);
      }
      q2q_core::client::OfflineResume::Complete(transfer) => {
        return finish_download(transfer, args.quiet, &mut stop).await;
      }
    }
  }
  fs::create_dir_all(&home)?;
  let identity_path = args
    .identity
    .clone()
    .unwrap_or_else(|| home.join("identity"));
  let _identity_lock = lock_identity(&identity_path)?;
  match &args.command {
    Command::Publish { file, .. } => {
      let track = args.track.as_deref().unwrap_or(DEFAULT_TRACK_URL);
      validate_publish_track_url(track)?;
      let (client, interrupted) =
        build_client(track, &args, &identity_path, &home, &mut stop).await?;
      if interrupted {
        return Ok(Outcome::Interrupted);
      }
      if !args.quiet {
        eprintln!("Local peer: {}", client.peer_id());
      }
      publish(
        &client,
        file,
        publish_seed_path
          .as_deref()
          .expect("publish seed path resolved"),
        args.quiet,
        &mut stop,
      )
      .await
    }
    Command::Download { resume, .. } => {
      let candidates = download_track_candidates(
        args.track.as_deref(),
        seed.as_ref().expect("download seed loaded"),
      );
      download_with_track_failover(
        &candidates,
        &args,
        &identity_path,
        &home,
        seed.as_ref().expect("download seed loaded"),
        download_output
          .as_deref()
          .expect("download output path resolved"),
        *resume || resume_from_checkpoint,
        offline_resume.as_ref(),
        &mut stop,
      )
      .await
    }
  }
}

async fn build_client(
  track: &str,
  args: &Args,
  identity_path: &Path,
  home: &Path,
  stop: &mut Stop,
) -> Result<(Client, bool)> {
  let mut builder = q2q_core::client::builder()
    .track_url(track)
    .danger_accept_invalid_certs(args.insecure)
    .storage_dir(home)
    .identity_file(identity_path)
    .max_download_peers(args.download_peers.into())
    .max_upload_peers(args.upload_peers.into());
  if let Some(rate) = args.upload_rate {
    builder = builder.upload_rate_limit(rate);
  }
  initialize(async { Ok(builder.build().await?) }, stop).await
}

fn download_track_candidates(preferred: Option<&str>, seed: &Seed) -> Vec<String> {
  let mut candidates = Vec::new();
  if let Some(preferred) = preferred {
    push_track_candidate(&mut candidates, preferred);
  }
  for track in &seed.track_list {
    if is_http_track_url(track) {
      push_track_candidate(&mut candidates, track);
    }
  }
  push_track_candidate(&mut candidates, DEFAULT_TRACK_URL);
  candidates
}

fn push_track_candidate(candidates: &mut Vec<String>, value: &str) {
  let key = Url::parse(value)
    .map(|url| url.to_string())
    .unwrap_or_else(|_| value.to_owned());
  if candidates.iter().all(|candidate| {
    Url::parse(candidate)
      .map(|url| url.to_string())
      .unwrap_or_else(|_| candidate.clone())
      != key
  }) {
    candidates.push(value.to_owned());
  }
}

fn is_http_track_url(value: &str) -> bool {
  Url::parse(value)
    .ok()
    .is_some_and(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
}

fn default_seed_path(file: &Path) -> io::Result<PathBuf> {
  let name = file.file_name().ok_or_else(|| {
    io::Error::new(
      io::ErrorKind::InvalidInput,
      "source path must include a filename to derive the default seed path",
    )
  })?;
  let mut seed_name = name.to_os_string();
  seed_name.push(".q2q");
  Ok(file.with_file_name(seed_name))
}

fn default_download_path(seed_path: &Path, seed: &Seed) -> io::Result<PathBuf> {
  if let Some(name) = seed
    .info
    .name
    .as_deref()
    .filter(|name| is_safe_filename(name))
  {
    return Ok(PathBuf::from(name));
  }

  let stem = seed_path
    .file_stem()
    .and_then(|name| name.to_str())
    .filter(|name| is_safe_filename(name))
    .ok_or_else(|| {
      io::Error::new(
        io::ErrorKind::InvalidInput,
        "seed has no safe filename; pass --output to choose a destination",
      )
    })?;
  Ok(PathBuf::from(stem))
}

fn is_safe_filename(name: &str) -> bool {
  if name.is_empty()
    || name == "."
    || name == ".."
    || name.ends_with([' ', '.'])
    || name.chars().any(|character| {
      character.is_control()
        || matches!(
          character,
          '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*'
        )
    })
  {
    return false;
  }

  let mut components = Path::new(name).components();
  if !matches!(components.next(), Some(std::path::Component::Normal(_)))
    || components.next().is_some()
  {
    return false;
  }

  let device = name
    .split('.')
    .next()
    .unwrap_or_default()
    .trim_end_matches([' ', '.'])
    .to_ascii_uppercase();
  if matches!(device.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
    return false;
  }
  ["COM", "LPT"].iter().all(|prefix| {
    !(device.len() == 4
      && device.starts_with(prefix)
      && matches!(device.as_bytes()[3], b'1'..=b'9'))
  })
}

fn resolve_home() -> io::Result<PathBuf> {
  if let Some(path) = env::var_os("Q2Q_HOME") {
    if path.is_empty() {
      return Err(io::Error::other("Q2Q_HOME must not be empty"));
    }
    return Ok(PathBuf::from(path));
  }

  #[cfg(target_os = "windows")]
  {
    if let Some(path) = nonempty_env("LOCALAPPDATA") {
      return Ok(path.join("Q2Q"));
    }
    if let Some(path) = nonempty_env("USERPROFILE") {
      return Ok(path.join("AppData").join("Local").join("Q2Q"));
    }
  }

  #[cfg(target_os = "macos")]
  if let Some(path) = nonempty_env("HOME") {
    return Ok(path.join("Library").join("Application Support").join("Q2Q"));
  }

  #[cfg(all(unix, not(target_os = "macos")))]
  {
    if let Some(path) = nonempty_env("XDG_STATE_HOME")
      && path.is_absolute()
    {
      return Ok(path.join("q2q"));
    }
    if let Some(path) = nonempty_env("HOME") {
      return Ok(path.join(".local").join("state").join("q2q"));
    }
  }

  Err(io::Error::other(
    "cannot determine the Q2Q state directory; set Q2Q_HOME",
  ))
}

fn nonempty_env(name: &str) -> Option<PathBuf> {
  env::var_os(name)
    .filter(|value| !value.is_empty())
    .map(PathBuf::from)
}

fn lock_identity(identity_path: &Path) -> io::Result<File> {
  let parent = identity_path
    .parent()
    .filter(|parent| !parent.as_os_str().is_empty())
    .unwrap_or(Path::new("."));
  fs::create_dir_all(parent)?;
  let mut lock_name = identity_path.as_os_str().to_os_string();
  lock_name.push(".q2q.lock");
  let mut options = OpenOptions::new();
  options.create(true).truncate(false).read(true).write(true);
  #[cfg(unix)]
  {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
  }
  let lock = options.open(PathBuf::from(lock_name))?;
  lock.try_lock().map_err(|error| {
    io::Error::other(format!(
      "could not lock this Q2Q identity; another CLI process may already be using it. \
       Set a separate Q2Q_HOME or Q2Q_IDENTITY_FILE for each concurrent client ({error})"
    ))
  })?;
  Ok(lock)
}

fn validate_publish_track_url(value: &str) -> io::Result<()> {
  let url =
    Url::parse(value).map_err(|error| io::Error::other(format!("invalid Track URL: {error}")))?;
  if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
    return Err(io::Error::other(
      "Track URL must be HTTP(S) and include a host",
    ));
  }
  if !url.username().is_empty()
    || url.password().is_some()
    || url.query().is_some()
    || url.fragment().is_some()
  {
    return Err(io::Error::other(
      "Track URL used for publishing must not contain userinfo, query parameters, or a fragment because it is written into the seed",
    ));
  }
  Ok(())
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
  let (progress, updates) =
    tokio::sync::watch::channel(q2q_core::client::PublishProgress::default());
  let mut display = PreparationDisplay::new(file, quiet);
  let initialized = initialize_with_progress(
    async { Ok(client.publish_file_with_progress(file, progress).await?) },
    stop,
    updates,
    |update| display.update(update),
  )
  .await;
  display.finish();
  let (publication, interrupted) = initialized?;
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

async fn download_with_track_failover(
  candidates: &[String],
  args: &Args,
  identity_path: &Path,
  home: &Path,
  seed: &Seed,
  output: &Path,
  resume: bool,
  offline_resume: Option<&q2q_core::client::OfflineDownload>,
  stop: &mut Stop,
) -> Result<Outcome> {
  let mut last_error = None;
  for (index, track) in candidates.iter().enumerate() {
    let try_another_track = index + 1 < candidates.len();
    let (client, interrupted) = build_client(track, args, identity_path, home, stop).await?;
    if interrupted {
      return Ok(Outcome::Interrupted);
    }
    if index == 0 && !args.quiet {
      eprintln!("Local peer: {}", client.peer_id());
    }
    let attempt = initialize(
      async {
        Ok(if let Some(offline_resume) = offline_resume {
          if try_another_track {
            client
              .resume_download_from_offline_with_track_discovery(offline_resume)
              .await?
          } else {
            client.resume_download_from_offline(offline_resume).await?
          }
        } else if resume {
          if try_another_track {
            client
              .resume_download_with_track_discovery(seed, output)
              .await?
          } else {
            client.resume_download(seed, output).await?
          }
        } else {
          if try_another_track {
            client.download_with_track_discovery(seed, output).await?
          } else {
            client.download(seed, output).await?
          }
        })
      },
      stop,
    )
    .await;
    match attempt {
      Ok((transfer, true)) => {
        transfer.cancel();
        return Ok(Outcome::Interrupted);
      }
      Ok((transfer, false)) => return finish_download(transfer, args.quiet, stop).await,
      Err(error) if index + 1 < candidates.len() && is_retryable_track_failure(&error) => {
        if !args.quiet {
          eprintln!(
            "Track candidate {} failed during session setup or initial peer discovery, or returned no peers ({error}); trying candidate {}.",
            index + 1,
            index + 2,
          );
        }
        last_error = Some(error);
      }
      Err(error) => return Err(error),
    }
  }
  Err(
    last_error.unwrap_or_else(|| Box::new(io::Error::other("no Track candidates were configured"))),
  )
}

fn is_retryable_track_failure(error: &Box<dyn std::error::Error + Send + Sync>) -> bool {
  error
    .downcast_ref::<q2q_core::client::TransferError>()
    .is_some_and(|error| {
      matches!(
        error,
        q2q_core::client::TransferError::Control(_)
          | q2q_core::client::TransferError::Timeout
          | q2q_core::client::TransferError::NoPeers
      )
    })
}

async fn finish_download(
  transfer: q2q_core::client::Transfer,
  quiet: bool,
  stop: &mut Stop,
) -> Result<Outcome> {
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
        eprintln!("Download cancelled; keep the destination and Q2Q_HOME directory to resume.");
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
