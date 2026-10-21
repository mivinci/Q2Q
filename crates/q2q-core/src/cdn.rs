//! P2P-first HTTP fallback. Every HTTP byte remains untrusted until root verification.

use crate::error::TransferError;
use crate::progress::{ActiveCdnRequest, TransferProgress};
use q2q_http::Executor;
use q2q_primitives::merkle::{self, Hash, ProofTree};
use q2q_primitives::seed::Seed;
use std::sync::Arc;
use tokio::sync::watch;
use url::Url;

fn error(message: impl Into<String>) -> TransferError {
  TransferError::Cdn(message.into())
}

fn http_error(e: q2q_http::Error) -> TransferError {
  error(e.to_string())
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

async fn leaf_tree(
  http: &Executor,
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
      let length = seed.info.leaf_count() * 32;
      let response = http
        .get(url)
        .expected_size(length as u64)
        .send()
        .await
        .map_err(http_error)?;
      let bytes = response.collect_exact(length).await.map_err(http_error)?;
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
  http: &Executor,
  url: Url,
  seed: &Seed,
  index: usize,
  progress: &watch::Sender<TransferProgress>,
) -> Result<Vec<u8>, TransferError> {
  let start = index as u64 * u64::from(seed.info.block_size);
  let length = crate::progress::block_len(seed.info.length, seed.info.block_size, index);
  let _active = ActiveCdnRequest::new(progress.clone());
  let response = http
    .get_range(url, start..start + length, seed.info.length)
    .send()
    .await
    .map_err(http_error)?;
  Ok(
    response
      .collect_exact(length as usize)
      .await
      .map_err(http_error)?
      .to_vec(),
  )
}

#[derive(Clone)]
pub(crate) struct Source {
  http: Executor,
  tree: Arc<ProofTree>,
  urls: Vec<Url>,
}
impl Source {
  pub(crate) async fn load(
    seed: &Seed,
    progress: &watch::Sender<TransferProgress>,
    http: Executor,
  ) -> Result<Self, TransferError> {
    let urls: Vec<_> = seed
      .cdn_list
      .iter()
      .filter_map(|source| http_url(source).ok())
      .collect();
    if urls.is_empty() {
      return Err(error("no usable CDN source"));
    }
    let tree = leaf_tree(&http, seed, progress).await?;
    Ok(Self {
      http,
      tree: Arc::new(tree),
      urls,
    })
  }
  pub(crate) fn mirrors(&self) -> impl Iterator<Item = Self> + '_ {
    let mut seen = std::collections::HashSet::new();
    self
      .urls
      .iter()
      .filter(move |url| seen.insert((*url).clone()))
      .map(|url| Self {
        http: self.http.clone(),
        tree: self.tree.clone(),
        urls: vec![url.clone()],
      })
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

#[cfg(test)]
pub(crate) use crate::download::cdn_all as download;

#[cfg(test)]
#[path = "../tests/cdn.rs"]
mod tests;
