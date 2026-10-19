//! Leaf hashes that have already been checked against a root.
//!
//! State, not computation — the tree maths lives in `merkle`. They are split
//! because the thing worth protecting here is an invariant rather than a
//! formula: nothing may enter a cache until it has been through a root
//! comparison that passed. Keeping that rule in its own module, with the single
//! entry point sitting right next to the private `absorb`, makes it hard to
//! break by accident.

use crate::merkle::{self, Hash, Proof};

/// Leaf hashes that have already been checked against a root.
///
/// The invariant that makes this safe: nothing enters the cache until it has
/// taken part in a root comparison that succeeded, or arrived as a full list
/// that rebuilds to `root` locally. A cache filled from unverified claims would
/// be a second trust root — every O(1) lookup after that would be comparing
/// against bytes a peer invented.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LeafCache {
  leaves: Vec<Option<Hash>>,
}

impl LeafCache {
  pub fn new(leaf_count: usize) -> Self {
    LeafCache {
      leaves: vec![None; leaf_count],
    }
  }

  pub fn leaf_count(&self) -> usize {
    self.leaves.len()
  }

  pub fn known(&self, index: usize) -> bool {
    matches!(self.leaves.get(index), Some(Some(_)))
  }

  pub fn known_count(&self) -> usize {
    self.leaves.iter().filter(|h| h.is_some()).count()
  }

  /// The cached leaf, if this index is known. Used when writing the cache out.
  pub fn leaf(&self, index: usize) -> Option<&Hash> {
    self.leaves.get(index)?.as_ref()
  }

  /// Rebuild a cache from persisted slots. **The caller must have established
  /// that these leaves belong to the same root** — a single slot cannot vouch
  /// for itself, only the whole table can (see `LeafCache::root`).
  pub fn from_slots(slots: Vec<Option<Hash>>) -> Self {
    LeafCache { leaves: slots }
  }

  /// O(1) check for a block whose leaf is cached. `None` means this index is
  /// not cached yet and the caller has to go get a proof.
  pub fn verify(&self, index: usize, block: &[u8]) -> Option<bool> {
    let claimed = self.leaves.get(index)?.as_ref()?;
    Some(merkle::leaf_hash(block) == *claimed)
  }

  /// A whole list at once — the CDN / `proof_list` path of spec 5.1. It is
  /// rebuilt into a root and compared before any of it is trusted, which is the
  /// same code the proof path uses.
  pub fn from_leaf_hashes(leaves: &[Hash], root_hash: &Hash) -> Option<Self> {
    if leaves.is_empty() || &merkle::root(leaves) != root_hash {
      return None;
    }
    Some(LeafCache {
      leaves: leaves.iter().map(|h| Some(*h)).collect(),
    })
  }

  /// The root this cache rebuilds, once every leaf is known. A cache coming
  /// back off disk can be checked against `info.root` with this.
  pub fn root(&self) -> Option<Hash> {
    // 有一个未知就收不出整表
    let leaves = self.leaves.iter().copied().collect::<Option<Vec<Hash>>>()?;
    Some(merkle::root(&leaves))
  }

  /// Private on purpose: the only way in is `verify_range_into_cache`, which
  /// absorbs only after the root comparison passed.
  fn absorb(&mut self, start: usize, leaf_hashes: &[Hash]) {
    for (i, h) in leaf_hashes.iter().enumerate() {
      if let Some(slot) = self.leaves.get_mut(start + i) {
        *slot = Some(*h);
      }
    }
  }
}

/// Verify a range and, only if it passed, remember its leaves.
///
/// Bundling the two is the whole point — it leaves no way to cache a proof
/// that has not been checked. spec 4.2.1 allows the uploader to omit leaf
/// hashes on short ranges; in that case nothing is cached and the next block
/// in the range still needs a proof.
pub fn verify_range_into_cache(
  blocks: &[&[u8]],
  start: usize,
  leaf_count: usize,
  proof: &Proof,
  root_hash: &Hash,
  cache: &mut LeafCache,
) -> bool {
  if !merkle::verify_range(blocks, start, leaf_count, proof, root_hash) {
    return false;
  }
  cache.absorb(start, &proof.leaf_hashes);
  true
}

#[cfg(test)]
mod tests {
  use super::*;

  fn leaves_of(n: usize, block_len: usize) -> (Vec<Vec<u8>>, Vec<Hash>) {
    let blocks: Vec<Vec<u8>> = (0..n).map(|i| vec![(i % 251) as u8; block_len]).collect();
    let leaves: Vec<Hash> = blocks.iter().map(|b| merkle::leaf_hash(b)).collect();
    (blocks, leaves)
  }

  fn range_of(blocks: &[Vec<u8>], start: usize, count: usize) -> Vec<&[u8]> {
    blocks[start..start + count]
      .iter()
      .map(|b| b.as_slice())
      .collect()
  }

  #[test]
  fn a_cached_leaf_answers_without_a_proof() {
    let (blocks, leaves) = leaves_of(4, 32);
    let root_hash = merkle::root(&leaves);
    let mut cache = LeafCache::new(4);
    let proof = merkle::prove_range(&leaves, 1, 2);

    assert!(verify_range_into_cache(
      &range_of(&blocks, 1, 2),
      1,
      4,
      &proof,
      &root_hash,
      &mut cache
    ));
    assert_eq!(cache.known_count(), 2);
    // O(1) 查表，不必再走一遍路径
    assert_eq!(cache.verify(1, &blocks[1]), Some(true));
    assert_eq!(cache.verify(2, &blocks[2]), Some(true));
    assert_eq!(cache.verify(1, &blocks[0]), Some(false)); // 拿错了块
    assert_eq!(cache.verify(3, &blocks[3]), None); // 这块还没缓存过
  }

  #[test]
  fn a_failed_verification_caches_nothing() {
    let (blocks, leaves) = leaves_of(4, 32);
    let root_hash = merkle::root(&leaves);
    let mut cache = LeafCache::new(4);
    let proof = merkle::prove_range(&leaves, 1, 2);

    let mut bad = blocks.clone();
    bad[2][0] ^= 0xff;
    assert!(!verify_range_into_cache(
      &range_of(&bad, 1, 2),
      1,
      4,
      &proof,
      &root_hash,
      &mut cache
    ));
    assert_eq!(cache.known_count(), 0);
  }

  #[test]
  fn a_leaf_list_only_loads_if_it_rebuilds_the_root() {
    let (_, leaves) = leaves_of(4, 32);
    let root_hash = merkle::root(&leaves);
    let cache = LeafCache::from_leaf_hashes(&leaves, &root_hash).unwrap();
    assert_eq!(cache.known_count(), 4);
    assert_eq!(cache.root(), Some(root_hash));

    // 一份对不上 root 的列表：一个都不许装进去
    let mut other = leaves.clone();
    other[2][0] ^= 0xff;
    assert!(LeafCache::from_leaf_hashes(&other, &root_hash).is_none());
  }

  #[test]
  fn an_incomplete_cache_has_no_root() {
    let (blocks, leaves) = leaves_of(4, 32);
    let root_hash = merkle::root(&leaves);
    let mut cache = LeafCache::new(4);
    let proof = merkle::prove_range(&leaves, 1, 1);
    assert!(verify_range_into_cache(
      &range_of(&blocks, 1, 1),
      1,
      4,
      &proof,
      &root_hash,
      &mut cache
    ));
    // 只知其一，重建不出整棵树的根
    assert_eq!(cache.root(), None);
  }

  #[test]
  fn absorb_stays_inside_the_table() {
    // 区间尾部越界时丢掉多余的，而不是 panic 或者把表撑长
    let (_, leaves) = leaves_of(4, 32);
    let mut cache = LeafCache::new(2);
    cache.absorb(1, &leaves);
    assert_eq!(cache.leaf_count(), 2);
    assert!(cache.known(1));
    assert!(!cache.known(0));
  }
}
