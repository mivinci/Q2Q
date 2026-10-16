//! Local storage for a single content file: block states, piece bitmaps and
//! in-place writes.
//!
//! Pieces go straight to their final offset — there is no "download to a temp
//! file, verify, then copy over" step, so a partially downloaded file is a
//! sparse file as far as the filesystem is concerned.

use crate::bencode::{self, BValue, Dict};
use crate::merkle::{self, Hash, Proof};
use std::fs::File as FsFile;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const PIECE_SIZE: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockState {
  Missing,
  Partial,
  Downloaded,
  Verified,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
  Io(String),
  BlockOutOfRange(usize),
  PieceOutOfRange(usize),
  WrongDataLength { expected: usize, got: usize },
  NotDownloadable(BlockState),
  MalformedProgress,
  ProgressMismatch,
}

impl std::fmt::Display for StoreError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      StoreError::Io(e) => write!(f, "io error: {e}"),
      StoreError::BlockOutOfRange(i) => write!(f, "block out of range: {i}"),
      StoreError::PieceOutOfRange(i) => write!(f, "piece out of range: {i}"),
      StoreError::WrongDataLength { expected, got } => {
        write!(f, "expected {expected} bytes, got {got}")
      }
      StoreError::NotDownloadable(s) => write!(f, "block is {s:?}, nothing to verify"),
      StoreError::MalformedProgress => f.write_str("progress file is malformed"),
      StoreError::ProgressMismatch => f.write_str("progress file belongs to a different file"),
    }
  }
}

impl std::error::Error for StoreError {}

impl From<std::io::Error> for StoreError {
  fn from(e: std::io::Error) -> Self {
    StoreError::Io(e.to_string())
  }
}

pub struct FileStore {
  path: PathBuf,
  length: u64,
  block_size: u32,
  states: Vec<BlockState>,
  /// One bitmap per block, held as u64 words.
  bitmaps: Vec<Vec<u64>>,
}

impl FileStore {
  pub fn create(path: impl AsRef<Path>, length: u64, block_size: u32) -> Result<Self, StoreError> {
    let path = path.as_ref().to_path_buf();
    let f = FsFile::create(&path)?;
    if length > 0 {
      f.set_len(length)?;
    }
    let block_count = length.div_ceil(block_size as u64) as usize;
    let mut store = FileStore {
      path,
      length,
      block_size,
      states: vec![BlockState::Missing; block_count],
      bitmaps: Vec::new(),
    };
    store.bitmaps = (0..block_count).map(|i| store.empty_bitmap(i)).collect();
    Ok(store)
  }

  pub fn block_count(&self) -> usize {
    self.states.len()
  }

  /// Byte length of one block; the last one may be short.
  pub fn block_len(&self, block: usize) -> usize {
    let start = block as u64 * self.block_size as u64;
    (self.length - start).min(self.block_size as u64) as usize
  }

  /// Number of pieces in a block; the last block may hold fewer.
  pub fn piece_count(&self, block: usize) -> usize {
    self.block_len(block).div_ceil(PIECE_SIZE)
  }

  fn empty_bitmap(&self, block: usize) -> Vec<u64> {
    vec![0u64; self.piece_count(block).div_ceil(64)]
  }

  pub fn state(&self, block: usize) -> Result<BlockState, StoreError> {
    self
      .states
      .get(block)
      .copied()
      .ok_or(StoreError::BlockOutOfRange(block))
  }

  /// Write one piece at its final offset and advance the block state.
  pub fn write_piece(
    &mut self,
    block: usize,
    piece: usize,
    data: &[u8],
  ) -> Result<BlockState, StoreError> {
    if block >= self.block_count() {
      return Err(StoreError::BlockOutOfRange(block));
    }
    let piece_count = self.piece_count(block);
    if piece >= piece_count {
      return Err(StoreError::PieceOutOfRange(piece));
    }
    let expected = self.piece_len(block, piece);
    if data.len() != expected {
      return Err(StoreError::WrongDataLength {
        expected,
        got: data.len(),
      });
    }

    let offset = block as u64 * self.block_size as u64 + piece as u64 * PIECE_SIZE as u64;
    let mut f = FsFile::options().write(true).open(&self.path)?;
    f.seek(SeekFrom::Start(offset))?;
    f.write_all(data)?;

    self.set_bit(block, piece, true);
    if self.states[block] == BlockState::Missing {
      self.states[block] = BlockState::Partial;
    }
    if self.all_pieces_present(block) && self.states[block] == BlockState::Partial {
      self.states[block] = BlockState::Downloaded;
    }
    Ok(self.states[block])
  }

  /// Bytes in one piece; only the very last piece of the file can be short.
  pub fn piece_len(&self, block: usize, piece: usize) -> usize {
    let block_len = self.block_len(block);
    let start = piece * PIECE_SIZE;
    (block_len - start).min(PIECE_SIZE)
  }

  fn set_bit(&mut self, block: usize, piece: usize, value: bool) {
    let word = piece / 64;
    let bit = piece % 64;
    if value {
      self.bitmaps[block][word] |= 1 << bit;
    } else {
      self.bitmaps[block][word] &= !(1 << bit);
    }
  }

  fn get_bit(&self, block: usize, piece: usize) -> bool {
    let word = piece / 64;
    let bit = piece % 64;
    self.bitmaps[block][word] & (1 << bit) != 0
  }

  fn all_pieces_present(&self, block: usize) -> bool {
    (0..self.piece_count(block)).all(|p| self.get_bit(block, p))
  }

  /// Read a block back and check it against the tree root.
  ///
  /// A failure sends the block back to `Partial` and clears its bitmap: a root
  /// mismatch usually means the data itself came from a bad peer, so keeping it
  /// would only fail again.
  pub fn verify(
    &mut self,
    block: usize,
    proof: &Proof,
    root: &Hash,
  ) -> Result<BlockState, StoreError> {
    match self.state(block)? {
      BlockState::Verified => return Ok(BlockState::Verified),
      BlockState::Downloaded => {}
      other => return Err(StoreError::NotDownloadable(other)),
    }

    let data = self.read_block(block)?;
    let slice = [data.as_slice()];
    if merkle::verify_range(&slice, block, self.block_count(), proof, root) {
      self.states[block] = BlockState::Verified;
      Ok(BlockState::Verified)
    } else {
      self.states[block] = BlockState::Partial;
      self.bitmaps[block] = self.empty_bitmap(block);
      Ok(BlockState::Partial)
    }
  }

  pub fn read_block(&self, block: usize) -> Result<Vec<u8>, StoreError> {
    if block >= self.block_count() {
      return Err(StoreError::BlockOutOfRange(block));
    }
    let offset = block as u64 * self.block_size as u64;
    let len = self.block_len(block);
    let mut buf = vec![0u8; len];
    let mut f = FsFile::open(&self.path)?;
    f.seek(SeekFrom::Start(offset))?;
    f.read_exact(&mut buf)?;
    Ok(buf)
  }

  /// Bitfield for the wire: only verified blocks are set.
  pub fn verified_bitfield(&self) -> Vec<u8> {
    let mut out = vec![0u8; self.block_count().div_ceil(8)];
    for (i, s) in self.states.iter().enumerate() {
      if *s == BlockState::Verified {
        out[i / 8] |= 1 << (7 - (i % 8));
      }
    }
    out
  }

  pub fn progress_path(&self) -> PathBuf {
    let mut p = self.path.clone().into_os_string();
    p.push(".q2q");
    PathBuf::from(p)
  }

  pub fn save_progress(&self) -> Result<(), StoreError> {
    let mut states = Vec::with_capacity(self.block_count());
    let mut maps = Vec::with_capacity(self.block_count());
    for (i, s) in self.states.iter().enumerate() {
      states.push(BValue::Int(*s as i64));
      let bytes: Vec<u8> = self.bitmaps[i]
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect();
      maps.push(BValue::Bytes(bytes));
    }

    let mut d = Dict::new();
    d.insert(b"v".to_vec(), BValue::Int(1));
    d.insert(b"length".to_vec(), BValue::Int(self.length as i64));
    d.insert(b"block_size".to_vec(), BValue::Int(self.block_size as i64));
    d.insert(b"states".to_vec(), BValue::List(states));
    d.insert(b"bitmaps".to_vec(), BValue::List(maps));

    let mut f = FsFile::create(self.progress_path())?;
    f.write_all(&BValue::Dict(d).encode())?;
    Ok(())
  }

  /// Restore progress saved by `save_progress`. Anything that no longer matches
  /// the file is rejected rather than silently trusted.
  pub fn load_progress(&mut self) -> Result<(), StoreError> {
    let bytes = std::fs::read(self.progress_path())?;
    let value = bencode::decode(&bytes).map_err(|_| StoreError::MalformedProgress)?;
    let d = match &value {
      BValue::Dict(d) => d,
      _ => return Err(StoreError::MalformedProgress),
    };

    let length = match d.get(&b"length"[..]) {
      Some(BValue::Int(n)) => *n as u64,
      _ => return Err(StoreError::MalformedProgress),
    };
    let block_size = match d.get(&b"block_size"[..]) {
      Some(BValue::Int(n)) => *n as u32,
      _ => return Err(StoreError::MalformedProgress),
    };
    if length != self.length || block_size != self.block_size {
      return Err(StoreError::ProgressMismatch);
    }

    let states = match d.get(&b"states"[..]) {
      Some(BValue::List(v)) => v,
      _ => return Err(StoreError::MalformedProgress),
    };
    let maps = match d.get(&b"bitmaps"[..]) {
      Some(BValue::List(v)) => v,
      _ => return Err(StoreError::MalformedProgress),
    };
    if states.len() != self.block_count() || maps.len() != self.block_count() {
      return Err(StoreError::MalformedProgress);
    }

    for (i, s) in states.iter().enumerate() {
      let n = match s {
        BValue::Int(n) => *n,
        _ => return Err(StoreError::MalformedProgress),
      };
      self.states[i] = match n {
        0 => BlockState::Missing,
        1 => BlockState::Partial,
        2 => BlockState::Downloaded,
        _ => BlockState::Verified,
      };
    }
    for (i, m) in maps.iter().enumerate() {
      let bytes = match m {
        BValue::Bytes(b) => b,
        _ => return Err(StoreError::MalformedProgress),
      };
      let expected = self.empty_bitmap(i).len() * 8;
      if bytes.len() != expected {
        return Err(StoreError::MalformedProgress);
      }
      let mut words = vec![0u64; expected / 8];
      for (w, chunk) in words.iter_mut().zip(bytes.chunks(8)) {
        let mut arr = [0u8; 8];
        arr.copy_from_slice(chunk);
        *w = u64::from_le_bytes(arr);
      }
      self.bitmaps[i] = words;
    }
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::merkle::leaf_hash;
  use tempfile::TempDir;

  fn tree_of(blocks: &[Vec<u8>]) -> (Vec<Hash>, Hash) {
    let leaves: Vec<Hash> = blocks.iter().map(|b| leaf_hash(b)).collect();
    let root = merkle::root(&leaves);
    (leaves, root)
  }

  #[test]
  fn block_lengths_and_piece_counts() {
    let dir = TempDir::new().unwrap();
    let store = FileStore::create(dir.path().join("f"), 700_000, 256 * 1024).unwrap();
    assert_eq!(store.block_count(), 3);
    assert_eq!(store.block_len(0), 256 * 1024);
    assert_eq!(store.piece_count(0), 256);
    // 700_000 - 2*262_144 = 175_712
    assert_eq!(store.block_len(2), 175_712);
    assert_eq!(store.piece_count(2), 175_712 / PIECE_SIZE + 1);
    // the final piece holds the remainder
    let last = store.piece_count(2) - 1;
    assert_eq!(store.piece_len(2, last), 175_712 % PIECE_SIZE);
  }

  #[test]
  fn state_advances_from_missing_to_verified() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("f");
    let block_size = 256 * 1024u32;
    let mut store = FileStore::create(&path, 4 * PIECE_SIZE as u64, block_size).unwrap();
    let blocks = vec![vec![7u8; 4 * PIECE_SIZE]];
    let (leaves, root) = tree_of(&blocks);

    assert_eq!(store.state(0).unwrap(), BlockState::Missing);
    for p in 0..3 {
      let start = p * PIECE_SIZE;
      let chunk = blocks[0][start..start + PIECE_SIZE].to_vec();
      let s = store.write_piece(0, p, &chunk).unwrap();
      assert_eq!(s, BlockState::Partial);
    }
    let start = 3 * PIECE_SIZE;
    let last = blocks[0][start..].to_vec();
    assert_eq!(
      store.write_piece(0, 3, &last).unwrap(),
      BlockState::Downloaded
    );

    let proof = merkle::prove(&leaves, 0);
    assert_eq!(
      store.verify(0, &proof, &root).unwrap(),
      BlockState::Verified
    );
    assert_eq!(store.state(0).unwrap(), BlockState::Verified);
  }

  #[test]
  fn writes_land_at_the_final_offset() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("f");
    let mut store = FileStore::create(&path, 3 * PIECE_SIZE as u64, 256 * 1024).unwrap();
    // first and third piece only; the gap must stay zeroed
    store.write_piece(0, 0, &[1u8; PIECE_SIZE]).unwrap();
    store.write_piece(0, 2, &[3u8; PIECE_SIZE]).unwrap();

    let on_disk = std::fs::read(&path).unwrap();
    assert_eq!(&on_disk[..PIECE_SIZE], &[1u8; PIECE_SIZE][..]);
    assert_eq!(&on_disk[PIECE_SIZE..2 * PIECE_SIZE], &[0u8; PIECE_SIZE][..]);
    assert_eq!(&on_disk[2 * PIECE_SIZE..], &[3u8; PIECE_SIZE][..]);
  }

  #[test]
  fn failed_verification_clears_the_bitmap() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("f");
    let mut store = FileStore::create(&path, 2 * PIECE_SIZE as u64, 256 * 1024).unwrap();
    let good = [vec![5u8; 2 * PIECE_SIZE]];

    for p in 0..2 {
      let start = p * PIECE_SIZE;
      store
        .write_piece(0, p, &good[0][start..start + PIECE_SIZE])
        .unwrap();
    }
    assert_eq!(store.state(0).unwrap(), BlockState::Downloaded);

    let proof = merkle::prove(&[leaf_hash(&good[0])], 0);
    // wrong root on purpose
    assert_eq!(
      store.verify(0, &proof, &[0xff; 32]).unwrap(),
      BlockState::Partial
    );
    assert_eq!(store.state(0).unwrap(), BlockState::Partial);
    // bitmap cleared: writing the pieces again is required
    assert_eq!(
      store.write_piece(0, 0, &good[0][..PIECE_SIZE]).unwrap(),
      BlockState::Partial
    );
  }

  #[test]
  fn bitfield_only_marks_verified_blocks() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("f");
    let mut store = FileStore::create(&path, 3 * 256 * 1024, 256 * 1024).unwrap();
    let blocks: Vec<Vec<u8>> = (0..3).map(|i| vec![i as u8; 256 * 1024]).collect();
    let (leaves, root) = tree_of(&blocks);

    // block 0 fully written but not verified, block 1 verified
    for p in 0..store.piece_count(0) {
      let start = p * PIECE_SIZE;
      let chunk = blocks[0][start..start + PIECE_SIZE].to_vec();
      store.write_piece(0, p, &chunk).unwrap();
    }
    for p in 0..store.piece_count(1) {
      let start = p * PIECE_SIZE;
      let chunk = blocks[1][start..start + PIECE_SIZE].to_vec();
      store.write_piece(1, p, &chunk).unwrap();
    }
    let proof1 = merkle::prove(&leaves, 1);
    store.verify(1, &proof1, &root).unwrap();

    let bf = store.verified_bitfield();
    assert_eq!(bf.len(), 1);
    assert_eq!(bf[0] & 0b0100_0000, 0b0100_0000); // block 1
    assert_eq!(bf[0] & 0b1000_0000, 0); // block 0 still unverified
  }

  #[test]
  fn progress_survives_a_reopen() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("f");
    let block_size = 256 * 1024u32;
    let mut store = FileStore::create(&path, 4 * PIECE_SIZE as u64, block_size).unwrap();
    store.write_piece(0, 0, &[9u8; PIECE_SIZE]).unwrap();
    store.save_progress().unwrap();

    let mut reopened = FileStore::create(&path, 4 * PIECE_SIZE as u64, block_size).unwrap();
    reopened.load_progress().unwrap();
    assert_eq!(reopened.state(0).unwrap(), BlockState::Partial);
    assert_eq!(reopened.piece_count(0), store.piece_count(0));
  }

  #[test]
  fn progress_for_another_file_is_rejected() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("f");
    let store = FileStore::create(&path, 4 * PIECE_SIZE as u64, 256 * 1024).unwrap();
    store.save_progress().unwrap();

    let mut other = FileStore::create(&path, 8 * PIECE_SIZE as u64, 256 * 1024).unwrap();
    assert_eq!(other.load_progress(), Err(StoreError::ProgressMismatch));
  }

  #[test]
  fn rejects_out_of_range_writes() {
    let dir = TempDir::new().unwrap();
    let mut store = FileStore::create(dir.path().join("f"), 1024, 256 * 1024).unwrap();
    assert_eq!(
      store.write_piece(1, 0, &[0u8; 1]),
      Err(StoreError::BlockOutOfRange(1))
    );
    assert_eq!(
      store.write_piece(0, 1, &[0u8; 1]),
      Err(StoreError::PieceOutOfRange(1))
    );
    assert_eq!(
      store.write_piece(0, 0, &[0u8; 10]),
      Err(StoreError::WrongDataLength {
        expected: 1024,
        got: 10
      })
    );
  }
}
