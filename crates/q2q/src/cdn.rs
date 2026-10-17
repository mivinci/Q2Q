//! P2P-first HTTP fallback. Every HTTP byte remains untrusted until root verification.

use crate::download_store::DownloadStore;
use crate::progress::{ActiveCdnRequest, TransferProgress};
use crate::scheduler::WorkQueue;
use crate::transfer::{TransferError, destination_complete};
use q2q_core::merkle::{self, Hash, ProofTree};
use q2q_core::seed::Seed;
use reqwest::header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_RANGE, RANGE};
use reqwest::{Client, Response, StatusCode};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinSet;
use url::Url;

fn error(message: impl Into<String>) -> TransferError {
  TransferError::Cdn(message.into())
}

fn http_error(e: reqwest::Error) -> TransferError {
  error(e.without_url().to_string())
}

fn http_url(value: &str) -> Result<Url, TransferError> {
  let url = Url::parse(value).map_err(|_| error("invalid HTTP source URL"))?;
  if !matches!(url.scheme(), "http" | "https")
    || url.host_str().is_none()
    || !url.username().is_empty()
    || url.password().is_some()
  {
    return Err(error("source must be HTTP(S) without embedded credentials"));
  }
  Ok(url)
}

async fn exact_body(mut response: Response, length: usize) -> Result<Vec<u8>, TransferError> {
  if response
    .headers()
    .get(CONTENT_ENCODING)
    .is_some_and(|v| !v.as_bytes().eq_ignore_ascii_case(b"identity"))
  {
    return Err(error("compressed HTTP representations are not supported"));
  }
  if response
    .content_length()
    .is_some_and(|n| n != length as u64)
  {
    return Err(error("HTTP response length does not match requested bytes"));
  }
  let mut bytes = Vec::with_capacity(length);
  while let Some(chunk) = response.chunk().await.map_err(http_error)? {
    if chunk.len() > length - bytes.len() {
      return Err(error("HTTP response exceeds requested bytes"));
    }
    bytes.extend_from_slice(&chunk);
  }
  if bytes.len() != length {
    return Err(error("HTTP response ended before requested bytes"));
  }
  Ok(bytes)
}

async fn leaf_tree(
  http: &Client,
  seed: &Seed,
  progress: &watch::Sender<TransferProgress>,
) -> Result<ProofTree, TransferError> {
  if let Some(leaves) = &seed.leaf_hashes {
    let tree = ProofTree::new(leaves);
    if tree.root() != seed.info.root {
      return Err(error("inline leaf table does not match root"));
    }
    return Ok(tree);
  }
  let mut last = error("CDN requires inline leaf_hashes or a valid proof_list source");
  for source in &seed.proof_list {
    let result = async {
      let url = http_url(source)?;
      let _active = ActiveCdnRequest::new(progress.clone());
      let response = http
        .get(url)
        .header(ACCEPT_ENCODING, "identity")
        .send()
        .await
        .map_err(http_error)?;
      if response.status() != StatusCode::OK {
        return Err(error("leaf table requires HTTP 200"));
      }
      let bytes = exact_body(response, seed.info.leaf_count() * 32).await?;
      let leaves: Vec<Hash> = bytes.as_chunks::<32>().0.to_vec();
      let tree = ProofTree::new(&leaves);
      if tree.root() != seed.info.root {
        return Err(error("downloaded leaf table does not match root"));
      }
      Ok(tree)
    }
    .await;
    match result {
      Ok(tree) => return Ok(tree),
      Err(e) => last = e,
    }
  }
  Err(last)
}

async fn block(
  http: &Client,
  url: Url,
  seed: &Seed,
  index: usize,
  progress: &watch::Sender<TransferProgress>,
) -> Result<Vec<u8>, TransferError> {
  let start = index as u64 * u64::from(seed.info.block_size);
  let length = crate::progress::block_len(seed.info.length, seed.info.block_size, index);
  let end = start + length - 1;
  let _active = ActiveCdnRequest::new(progress.clone());
  let response = http
    .get(url)
    .header(RANGE, format!("bytes={start}-{end}"))
    .header(ACCEPT_ENCODING, "identity")
    .send()
    .await
    .map_err(http_error)?;
  match response.status() {
    StatusCode::PARTIAL_CONTENT => {
      let expected = format!("bytes {start}-{end}/{}", seed.info.length);
      if response
        .headers()
        .get(CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        != Some(expected.as_str())
      {
        return Err(error(
          "Content-Range does not match requested block and file length",
        ));
      }
    }
    StatusCode::OK if start == 0 && length == seed.info.length => {}
    _ => {
      return Err(error(
        "block requires HTTP 206 (200 only for a complete single-block file)",
      ));
    }
  }
  exact_body(response, length as usize).await
}

pub(crate) struct Source {
  http: Client,
  tree: ProofTree,
  urls: Vec<Url>,
}
impl Source {
  pub(crate) async fn load(
    seed: &Seed,
    progress: &watch::Sender<TransferProgress>,
  ) -> Result<Self, TransferError> {
    let http = Client::builder()
      .timeout(Duration::from_secs(20))
      .connect_timeout(Duration::from_secs(5))
      .redirect(reqwest::redirect::Policy::limited(3))
      .build()
      .map_err(http_error)?;
    let urls: Vec<_> = seed
      .cdn_list
      .iter()
      .filter_map(|source| http_url(source).ok())
      .collect();
    if urls.is_empty() {
      return Err(error("no usable CDN source"));
    }
    let tree = leaf_tree(&http, seed, progress).await?;
    Ok(Self { http, tree, urls })
  }
  pub(crate) async fn fetch(
    &self,
    seed: &Seed,
    index: usize,
    progress: &watch::Sender<TransferProgress>,
  ) -> Result<(Vec<u8>, merkle::Proof), TransferError> {
    let proof = self.tree.proof(index).expect("validated block index");
    let mut last = error("no usable CDN source");
    for url in &self.urls {
      match block(&self.http, url.clone(), seed, index, progress).await {
        Ok(bytes)
          if merkle::verify(
            &bytes,
            index,
            seed.info.leaf_count(),
            &proof,
            &seed.info.root,
          ) =>
        {
          return Ok((bytes, proof));
        }
        Ok(_) => last = error("HTTP block does not match content root"),
        Err(e) => last = e,
      }
    }
    Err(last)
  }
}

pub(crate) async fn download(
  seed: Seed,
  store: Arc<Mutex<DownloadStore>>,
  concurrency: usize,
  progress: watch::Sender<TransferProgress>,
) -> Result<(), TransferError> {
  // A cancelled P2P worker may still be finishing a blocking commit. Snapshot
  // its durable state off the runtime thread and keep the same progress sender.
  let destination = store.clone();
  let bits = tokio::task::spawn_blocking(move || {
    let store = destination
      .lock()
      .map_err(|_| TransferError::Checkpoint("destination lock poisoned".into()))?;
    Ok::<_, TransferError>(store.verified_bitfield())
  })
  .await??;
  let queue = WorkQueue::new(&seed, &bits, progress.clone());
  if queue.complete() {
    return Ok(());
  }
  let http = Client::builder()
    .timeout(Duration::from_secs(20))
    .connect_timeout(Duration::from_secs(5))
    .redirect(reqwest::redirect::Policy::limited(3))
    .build()
    .map_err(http_error)?;
  let tree = Arc::new(leaf_tree(&http, &seed, &progress).await?);
  let seed = Arc::new(seed);
  let mut last = error("no usable CDN source");
  let mut seen = std::collections::HashSet::new();
  for source in &seed.cdn_list {
    if !seen.insert(source) {
      continue;
    }
    let url = match http_url(source) {
      Ok(url) => url,
      Err(e) => {
        last = e;
        continue;
      }
    };
    let mut workers = JoinSet::new();
    for _ in 0..concurrency {
      let http = http.clone();
      let url = url.clone();
      let seed = seed.clone();
      let tree = tree.clone();
      let store = store.clone();
      let queue = queue.clone();
      workers.spawn(async move {
        while let Some(lease) = queue.claim().await {
          let index = lease.index;
          let bytes = block(&http, url.clone(), &seed, index, &queue.progress).await?;
          let proof = tree
            .proof(index)
            .expect("queue index is within validated tree");
          if !merkle::verify(
            &bytes,
            index,
            seed.info.leaf_count(),
            &proof,
            &seed.info.root,
          ) {
            return Err(error("HTTP block does not match content root"));
          }
          let destination = store.clone();
          let root = seed.info.root;
          let task = lease.disk_task();
          tokio::task::spawn_blocking(move || {
            let _task = task;
            let mut store = destination
              .lock()
              .map_err(|_| TransferError::Checkpoint("destination lock poisoned".into()))?;
            store.commit(index, &bytes, proof, &root)?;
            lease.commit();
            Ok::<_, TransferError>(())
          })
          .await??;
        }
        Ok::<_, TransferError>(())
      });
    }
    while let Some(result) = workers.join_next().await {
      match result? {
        Ok(()) => {}
        Err(e @ TransferError::Cdn(_)) => last = e,
        Err(e) => return Err(e),
      }
    }
    if queue.complete() {
      return Ok(());
    }
  }
  if destination_complete(&store)? {
    Ok(())
  } else {
    Err(last)
  }
}

#[cfg(test)]
#[path = "../tests/cdn.rs"]
mod tests;
