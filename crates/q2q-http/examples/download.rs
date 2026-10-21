//! cargo run -p q2q-http --example download -- URL OUTPUT SIZE SHA1
use q2q_http::{Checksum, DownloadRequest, Downloader};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
  let mut args: Vec<_> = std::env::args().skip(1).collect();
  let resume = args.first().is_some_and(|arg| arg == "--resume");
  if resume {
    args.remove(0);
  }
  if args.len() != 4 {
    return Err("usage: download [--resume] URL OUTPUT SIZE SHA1".into());
  }
  let request = DownloadRequest::new(&args[1])
    .mirrors([&args[0]])
    .expected_size(args[2].parse()?)
    .checksum(Checksum::sha1(&args[3])?);
  let downloader = Downloader::builder().build()?;
  let task = if resume {
    downloader.resume(request).await?
  } else {
    downloader.download(request).await?
  };
  let mut updates = task.subscribe_progress();
  let reporter = tokio::spawn(async move {
    loop {
      let p = *updates.borrow_and_update();
      eprintln!(
        "{:?}: received {}, staged {}/{}, checked {}",
        p.state, p.received_bytes, p.staged_bytes, p.total_bytes, p.verification_bytes
      );
      if p.state.is_terminal() || updates.changed().await.is_err() {
        break;
      }
    }
  });
  let result = task.wait().await;
  reporter.await?;
  println!("{}", result?.display());
  Ok(())
}
