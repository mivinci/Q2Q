//! Merkle tree and proofs, in the RFC 6962 (Certificate Transparency) shape:
//! leaves and internal nodes hash with different domain prefixes, and a layer
//! holding an odd number of nodes promotes its last node instead of duplicating
//! it.
//!
//! Domain separation is mandatory: without it a 64-byte internal node could be
//! read back as a leaf input of the same length, which hands an attacker a
//! second preimage.

use sha2::{Digest, Sha256};
use std::collections::HashMap;

pub type Hash = [u8; 32];

const LEAF_PREFIX: u8 = 0x00;
const NODE_PREFIX: u8 = 0x01;

/// SHA-256(0x00 || block)
pub fn leaf_hash(block: &[u8]) -> Hash {
  let mut h = Sha256::new();
  h.update([LEAF_PREFIX]);
  h.update(block);
  h.finalize().into()
}

/// SHA-256(0x01 || left || right)
fn node_hash(left: &Hash, right: &Hash) -> Hash {
  let mut h = Sha256::new();
  h.update([NODE_PREFIX]);
  h.update(left);
  h.update(right);
  h.finalize().into()
}

/// A node the verifier needs but does not have. `level` counts from the leaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Node {
  pub level: u32,
  pub index: u64,
  pub hash: Hash,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Proof {
  /// Leaf hashes of the covered blocks. Carrying them lets the receiver cache
  /// the range and degrade later blocks in it to a table lookup.
  pub leaf_hashes: Vec<Hash>,
  /// The nodes above the covered leaves that are missing on the receiver side.
  pub nodes: Vec<Node>,
}

/// A tree over no leaves has no root. This stands in for it: SHA-256 never
/// outputs all zeroes, so it cannot be mistaken for a real root.
pub const EMPTY_ROOT: Hash = [0u8; 32];

/// Node count per layer, leaves first. The last entry is always 1 (the root).
fn layer_sizes(leaf_count: usize) -> Vec<usize> {
  let mut sizes = Vec::new();
  let mut size = leaf_count.max(1);
  loop {
    sizes.push(size);
    if size == 1 {
      return sizes;
    }
    size = size.div_ceil(2);
  }
}

fn build_layers(leaves: &[Hash]) -> Vec<Vec<Hash>> {
  let mut layer = leaves.to_vec();
  let mut layers = vec![layer.clone()];
  while layer.len() > 1 {
    layer = next_layer(&layer);
    layers.push(layer.clone());
  }
  layers
}

fn next_layer(prev: &[Hash]) -> Vec<Hash> {
  let mut next = Vec::with_capacity(prev.len().div_ceil(2));
  let mut i = 0;
  while i < prev.len() {
    if i + 1 < prev.len() {
      next.push(node_hash(&prev[i], &prev[i + 1]));
      i += 2;
    } else {
      next.push(prev[i]); // promoted, not duplicated
      i += 1;
    }
  }
  next
}

/// Root over the given leaf hashes. A single leaf is its own root; no leaves is
/// `EMPTY_ROOT`.
pub fn root(leaves: &[Hash]) -> Hash {
  match leaves {
    [] => EMPTY_ROOT,
    [only] => *only,
    _ => build_layers(leaves)
      .last()
      .and_then(|top| top.first().copied())
      .unwrap_or(EMPTY_ROOT),
  }
}

pub fn prove(leaves: &[Hash], index: usize) -> Proof {
  prove_range(leaves, index, 1)
}

/// Proof for the range `[start, start + count)`.
///
/// Only the siblings that fall outside the covered span are emitted, so the
/// number of upper-layer nodes does not grow with the length of the range.
pub fn prove_range(leaves: &[Hash], start: usize, count: usize) -> Proof {
  assert!(count > 0, "empty range proves nothing");
  assert!(start + count <= leaves.len(), "range outside the tree");

  let layers = build_layers(leaves);
  let end = start + count - 1;

  let mut nodes = Vec::new();
  for (level, layer) in layers.iter().enumerate() {
    let size = layer.len();
    let lo = start >> level;
    let hi = end >> level;
    for i in lo..=hi {
      let sibling = if i % 2 == 0 {
        // even index pairs with i+1, unless it is the promoted last node
        (i + 1 < size).then_some(i + 1)
      } else {
        Some(i - 1)
      };
      if let Some(s) = sibling {
        if s < lo || s > hi {
          nodes.push(Node {
            level: level as u32,
            index: s as u64,
            hash: layer[s],
          });
        }
      }
    }
  }

  Proof {
    leaf_hashes: leaves[start..start + count].to_vec(),
    nodes,
  }
}

/// Verify one block. `leaf_count` is the total number of leaves in the tree:
/// the verifier knows it from `length / block_size`, and the tree shape cannot
/// be reconstructed without it.
pub fn verify(
  block: &[u8],
  index: usize,
  leaf_count: usize,
  proof: &Proof,
  root_hash: &Hash,
) -> bool {
  verify_range(&[block], index, leaf_count, proof, root_hash)
}

pub fn verify_range(
  blocks: &[&[u8]],
  start: usize,
  leaf_count: usize,
  proof: &Proof,
  root_hash: &Hash,
) -> bool {
  if blocks.is_empty() || leaf_count == 0 {
    return false;
  }
  let sizes = layer_sizes(leaf_count);

  // 载荷带了叶子哈希时，它就是对数据的承诺（spec 4.2.1 的 base layer = 0）：
  // 必须跟区间一一对应，且每块算出来的叶子都得对上，否则字节本身就不对，
  // 不必再往上重建 root。上传方可以在短区间上省略它，那就走纯验证路径。
  if !proof.leaf_hashes.is_empty() && proof.leaf_hashes.len() != blocks.len() {
    return false;
  }

  let mut known: HashMap<(usize, u64), Hash> = HashMap::new();
  for (i, b) in blocks.iter().enumerate() {
    let leaf = leaf_hash(b);
    if let Some(claimed) = proof.leaf_hashes.get(i) {
      if leaf != *claimed {
        return false;
      }
    }
    known.insert((0, (start + i) as u64), leaf);
  }
  for n in &proof.nodes {
    if n.level as usize >= sizes.len() {
      return false;
    }
    known.insert((n.level as usize, n.index), n.hash);
  }

  let top = sizes.len() - 1;
  let mut memo = HashMap::new();
  match resolve(&known, &sizes, &mut memo, top, 0) {
    Some(h) => h == *root_hash,
    None => false,
  }
}

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

  /// O(1) check for a block whose leaf is cached. `None` means this index is
  /// not cached yet and the caller has to go get a proof.
  pub fn verify(&self, index: usize, block: &[u8]) -> Option<bool> {
    let claimed = self.leaves.get(index)?.as_ref()?;
    Some(leaf_hash(block) == *claimed)
  }

  /// A whole list at once — the CDN / `proof_list` path of spec 5.1. It is
  /// rebuilt into a root and compared before any of it is trusted, which is the
  /// same code the proof path uses.
  pub fn from_leaf_hashes(leaves: &[Hash], root_hash: &Hash) -> Option<Self> {
    if leaves.is_empty() || &root(leaves) != root_hash {
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
    Some(root(&leaves))
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
  if !verify_range(blocks, start, leaf_count, proof, root_hash) {
    return false;
  }
  cache.absorb(start, &proof.leaf_hashes);
  true
}

/// Rebuild a node from whatever is known, memoised so each node is hashed once.
fn resolve(
  known: &HashMap<(usize, u64), Hash>,
  sizes: &[usize],
  memo: &mut HashMap<(usize, u64), Hash>,
  level: usize,
  index: u64,
) -> Option<Hash> {
  let key = (level, index);
  if let Some(h) = memo.get(&key) {
    return Some(*h);
  }
  if let Some(h) = known.get(&key) {
    memo.insert(key, *h);
    return Some(*h);
  }
  if level == 0 {
    return None; // a leaf we do not have: the proof is incomplete
  }

  let left = resolve(known, sizes, memo, level - 1, index * 2)?;
  let right_index = index * 2 + 1;
  let hash = if (right_index as usize) < sizes[level - 1] {
    let right = resolve(known, sizes, memo, level - 1, right_index)?;
    node_hash(&left, &right)
  } else {
    left // last node of an odd layer was promoted
  };
  memo.insert(key, hash);
  Some(hash)
}

#[cfg(test)]
mod tests {
  use super::*;
  use proptest::prelude::*;

  fn leaves_of(n: usize, block_len: usize) -> (Vec<Vec<u8>>, Vec<Hash>) {
    let blocks: Vec<Vec<u8>> = (0..n).map(|i| vec![(i % 251) as u8; block_len]).collect();
    let leaves: Vec<Hash> = blocks.iter().map(|b| leaf_hash(b)).collect();
    (blocks, leaves)
  }

  #[test]
  fn an_empty_tree_has_a_defined_root() {
    // a zero-length file has no blocks; this must not panic
    assert_eq!(root(&[]), EMPTY_ROOT);
    assert_ne!(EMPTY_ROOT, leaf_hash(b""));
  }

  #[test]
  fn single_leaf_is_its_own_root() {
    let (blocks, leaves) = leaves_of(1, 16);
    assert_eq!(root(&leaves), leaf_hash(&blocks[0]));
    let proof = prove(&leaves, 0);
    assert!(proof.nodes.is_empty());
    assert!(verify(&blocks[0], 0, 1, &proof, &root(&leaves)));
  }

  #[test]
  fn odd_layer_promotes_instead_of_duplicating() {
    // n = 3: layer 1 is [node(0,1), leaf2]; leaf2 must not be hashed with itself
    let (blocks, leaves) = leaves_of(3, 8);
    let layers = build_layers(&leaves);
    assert_eq!(layers[1].len(), 2);
    assert_eq!(layers[1][1], leaves[2]);
    assert_ne!(layers[1][1], node_hash(&leaves[2], &leaves[2]));
    let _ = blocks;
  }

  #[test]
  fn every_index_proves_and_verifies() {
    for n in 1..=64usize {
      let (blocks, leaves) = leaves_of(n, 5);
      let root_hash = root(&leaves);
      for (i, block) in blocks.iter().enumerate() {
        let proof = prove(&leaves, i);
        assert!(verify(block, i, n, &proof, &root_hash), "n={n} index={i}");
      }
    }
  }

  #[test]
  fn a_cached_leaf_answers_without_a_proof() {
    let (blocks, leaves) = leaves_of(4, 32);
    let root_hash = root(&leaves);
    let mut cache = LeafCache::new(4);
    let proof = prove_range(&leaves, 1, 2);
    let range: Vec<&[u8]> = blocks[1..3].iter().map(|b| b.as_slice()).collect();

    assert!(verify_range_into_cache(
      &range, 1, 4, &proof, &root_hash, &mut cache
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
    let root_hash = root(&leaves);
    let mut cache = LeafCache::new(4);
    let proof = prove_range(&leaves, 1, 2);

    let mut bad = blocks.clone();
    bad[2][0] ^= 0xff;
    let range: Vec<&[u8]> = bad[1..3].iter().map(|b| b.as_slice()).collect();
    assert!(!verify_range_into_cache(
      &range, 1, 4, &proof, &root_hash, &mut cache
    ));
    assert_eq!(cache.known_count(), 0);
  }

  #[test]
  fn a_leaf_list_only_loads_if_it_rebuilds_the_root() {
    let (_, leaves) = leaves_of(4, 32);
    let root_hash = root(&leaves);
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
    let root_hash = root(&leaves);
    let mut cache = LeafCache::new(4);
    let proof = prove_range(&leaves, 1, 1);
    let range: Vec<&[u8]> = blocks[1..2].iter().map(|b| b.as_slice()).collect();
    assert!(verify_range_into_cache(
      &range, 1, 4, &proof, &root_hash, &mut cache
    ));
    // 只知其一，重建不出整棵树的根
    assert_eq!(cache.root(), None);
  }

  #[test]
  fn leaf_hashes_commit_to_the_bytes() {
    let (blocks, leaves) = leaves_of(4, 32);
    let root_hash = root(&leaves);
    let proof = prove_range(&leaves, 1, 2);
    let range: Vec<&[u8]> = blocks[1..3].iter().map(|b| b.as_slice()).collect();
    assert!(verify_range(&range, 1, 4, &proof, &root_hash));

    // 改一个字节：带了叶子哈希就该在叶子层当场拒绝
    let mut tampered = blocks.clone();
    tampered[2][0] ^= 0xff;
    let bad: Vec<&[u8]> = tampered[1..3].iter().map(|b| b.as_slice()).collect();
    assert!(!verify_range(&bad, 1, 4, &proof, &root_hash));
  }

  #[test]
  fn leaf_hashes_that_cover_only_part_of_the_range_are_rejected() {
    let (blocks, leaves) = leaves_of(4, 32);
    let root_hash = root(&leaves);
    let mut proof = prove_range(&leaves, 1, 2);
    proof.leaf_hashes.truncate(1); // 少一个：这份承诺不完整
    let range: Vec<&[u8]> = blocks[1..3].iter().map(|b| b.as_slice()).collect();
    assert!(!verify_range(&range, 1, 4, &proof, &root_hash));
  }

  #[test]
  fn a_proof_without_leaf_hashes_still_verifies() {
    // 上传方可以省略叶子哈希，此时走纯验证路径
    let (blocks, leaves) = leaves_of(4, 32);
    let root_hash = root(&leaves);
    let mut proof = prove_range(&leaves, 1, 2);
    proof.leaf_hashes.clear();
    let range: Vec<&[u8]> = blocks[1..3].iter().map(|b| b.as_slice()).collect();
    assert!(verify_range(&range, 1, 4, &proof, &root_hash));
  }

  #[test]
  fn every_range_proves_and_verifies() {
    for n in 1..=32usize {
      let (blocks, leaves) = leaves_of(n, 4);
      let root_hash = root(&leaves);
      for start in 0..n {
        for count in 1..=(n - start) {
          let proof = prove_range(&leaves, start, count);
          let slice: Vec<&[u8]> = blocks[start..start + count]
            .iter()
            .map(|b| b.as_slice())
            .collect();
          assert!(
            verify_range(&slice, start, n, &proof, &root_hash),
            "n={n} start={start} count={count}"
          );
        }
      }
    }
  }

  #[test]
  fn short_last_block_is_hashed_by_actual_length() {
    // last block shorter than the others; tree shape is unchanged
    let mut blocks: Vec<Vec<u8>> = vec![vec![1u8; 16], vec![2u8; 16]];
    blocks.push(vec![3u8; 3]);
    let leaves: Vec<Hash> = blocks.iter().map(|b| leaf_hash(b)).collect();
    let root_hash = root(&leaves);
    let proof = prove(&leaves, 2);
    let slice = [blocks[2].as_slice()];
    assert!(verify_range(&slice, 2, 3, &proof, &root_hash));
  }

  #[test]
  fn tampered_block_fails() {
    let (blocks, leaves) = leaves_of(8, 8);
    let root_hash = root(&leaves);
    let proof = prove(&leaves, 3);
    let mut bad = blocks[3].clone();
    bad[0] ^= 0xff;
    assert!(!verify(&bad, 3, 8, &proof, &root_hash));
  }

  #[test]
  fn tampered_proof_node_fails() {
    let (blocks, leaves) = leaves_of(8, 8);
    let root_hash = root(&leaves);
    let mut proof = prove(&leaves, 3);
    proof.nodes[0].hash[0] ^= 0xff;
    assert!(!verify(&blocks[3], 3, 8, &proof, &root_hash));
  }

  #[test]
  fn incomplete_proof_fails() {
    let (blocks, leaves) = leaves_of(8, 8);
    let root_hash = root(&leaves);
    let mut proof = prove(&leaves, 3);
    proof.nodes.pop();
    assert!(!verify(&blocks[3], 3, 8, &proof, &root_hash));
  }

  #[test]
  fn wrong_leaf_count_fails() {
    let (blocks, leaves) = leaves_of(8, 8);
    let root_hash = root(&leaves);
    let proof = prove(&leaves, 3);
    // same block, but a tree that claims a different number of leaves
    assert!(!verify(&blocks[3], 3, 9, &proof, &root_hash));
  }

  #[test]
  fn proof_size_stays_logarithmic() {
    let n = 8192;
    let (_, leaves) = leaves_of(n, 1);
    let bound = (usize::BITS - (n - 1).leading_zeros()) as usize;
    for i in [0usize, 1, n / 2, n - 1] {
      let proof = prove(&leaves, i);
      assert!(
        proof.nodes.len() <= bound,
        "index {i}: {} nodes",
        proof.nodes.len()
      );
    }
  }

  #[test]
  fn range_proof_does_not_grow_with_length() {
    let n = 1024;
    let (_, leaves) = leaves_of(n, 1);
    let short = prove_range(&leaves, 0, 1).nodes.len();
    let long = prove_range(&leaves, 0, 64).nodes.len();
    assert!(long <= short + 8, "short={short} long={long}");
  }

  proptest! {
    // Bounds are baked into the strategies rather than filtered afterwards:
    // independent free parameters get rejected so often that proptest aborts.
    #[test]
    fn arbitrary_tree_proves_every_index(
      (n, index, len) in (1usize..128).prop_flat_map(|n| (Just(n), 0usize..n, 1usize..40))
    ) {
      let (blocks, leaves) = leaves_of(n, len);
      let root_hash = root(&leaves);
      let proof = prove(&leaves, index);
      prop_assert!(verify(&blocks[index], index, n, &proof, &root_hash));
    }

    #[test]
    fn arbitrary_range_proves(
      (n, start, count) in (1usize..64).prop_flat_map(|n| {
        (0usize..n).prop_flat_map(move |start| (Just(n), Just(start), 1usize..=(n - start)))
      })
    ) {
      let (blocks, leaves) = leaves_of(n, 3);
      let root_hash = root(&leaves);
      let proof = prove_range(&leaves, start, count);
      let slice: Vec<&[u8]> = blocks[start..start + count].iter().map(|b| b.as_slice()).collect();
      prop_assert!(verify_range(&slice, start, n, &proof, &root_hash));
    }
  }
}
