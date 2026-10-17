//! Local storage for a single content file: block states, piece bitmaps and
//! in-place writes.
//!
//! Pieces go straight to their final offset — there is no "download to a temp
//! file, verify, then copy over" step, so a partially downloaded file is a
//! sparse file as far as the filesystem is concerned.

use crate::bencode::{self, BValue, Dict};
use crate::merkle::{self, Hash, Proof};
// block 大小的合法范围由 seed 定义（spec 5.2），store 不再自己另立一套
use crate::seed::{MAX_BLOCK_SIZE, MIN_BLOCK_SIZE};
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

/// One block: how much of it has landed, and where it stands.
///
/// State and bitmap live together because they are one thing. Keeping them in
/// parallel vectors means every access has to index twice and stay in step by
/// hand; here the association is structural.
#[derive(Debug)]
pub struct Block {
  state: BlockState,
  /// Byte length; only the last block is shorter than the store's block size.
  len: usize,
  /// One bit per piece, held as u64 words.
  bitmap: Vec<u64>,
}

impl Block {
  fn new(len: usize) -> Self {
    let pieces = len.div_ceil(PIECE_SIZE);
    Block {
      state: BlockState::Missing,
      len,
      bitmap: vec![0u64; pieces.div_ceil(64)],
    }
  }

  pub fn state(&self) -> BlockState {
    self.state
  }

  pub fn len(&self) -> usize {
    self.len
  }

  /// Only true for a block of a zero-length file; exists because `len` is
  /// public and callers expect the pair.
  pub fn is_empty(&self) -> bool {
    self.len == 0
  }

  pub fn piece_count(&self) -> usize {
    self.len.div_ceil(PIECE_SIZE)
  }

  /// Bytes in one piece. Only the last piece of a short block can be short.
  /// Out-of-range pieces give 0 — this is public, so it must not panic.
  pub fn piece_len(&self, piece: usize) -> usize {
    let start = piece * PIECE_SIZE;
    self.len.saturating_sub(start).min(PIECE_SIZE)
  }

  pub fn piece_present(&self, piece: usize) -> bool {
    if piece >= self.piece_count() {
      return false;
    }
    let (word, bit) = (piece / 64, piece % 64);
    self.bitmap[word] & (1 << bit) != 0
  }

  fn set_piece(&mut self, piece: usize, present: bool) {
    let (word, bit) = (piece / 64, piece % 64);
    if present {
      self.bitmap[word] |= 1 << bit;
    } else {
      self.bitmap[word] &= !(1 << bit);
    }
  }

  fn all_pieces_present(&self) -> bool {
    (0..self.piece_count()).all(|p| self.piece_present(p))
  }

  /// Record a piece and advance the state. Returns the state afterwards.
  fn mark_piece(&mut self, piece: usize) -> BlockState {
    self.set_piece(piece, true);
    if self.state == BlockState::Missing {
      self.state = BlockState::Partial;
    }
    // 往已校验的 block 里写 piece，字节已经跟 root 不一致了，不能再算 verified。
    // piece 齐了就落到 Downloaded —— 数据是全的，只是得重新校验一次。
    if self.state == BlockState::Verified {
      self.state = BlockState::Partial;
    }
    if self.state == BlockState::Partial && self.all_pieces_present() {
      self.state = BlockState::Downloaded;
    }
    self.state
  }

  fn mark_verified(&mut self) {
    self.state = BlockState::Verified;
  }

  /// Drop every piece. Called when verification fails: a root mismatch usually
  /// means the bytes themselves came from a bad peer, so keeping them would
  /// only fail again.
  fn reset(&mut self) {
    self.bitmap.fill(0);
    self.state = BlockState::Partial;
  }

  fn bitmap_bytes(&self) -> Vec<u8> {
    self.bitmap.iter().flat_map(|w| w.to_le_bytes()).collect()
  }

  fn set_bitmap_bytes(&mut self, bytes: &[u8]) -> bool {
    let expected = self.bitmap.len() * 8;
    if bytes.len() != expected {
      return false;
    }
    for (w, chunk) in self.bitmap.iter_mut().zip(bytes.chunks(8)) {
      let mut arr = [0u8; 8];
      arr.copy_from_slice(chunk);
      *w = u64::from_le_bytes(arr);
    }
    true
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
  Io(String),
  BlockOutOfRange(usize),
  PieceOutOfRange(usize),
  WrongDataLength {
    expected: usize,
    got: usize,
  },
  NotDownloadable(BlockState),
  /// 校验没通过：证明无效或算出来的 root 与 `info.root` 不符（spec 4.2）。
  /// 是一个 `Err` 而不是 `Ok(Partial)`——调用方要靠它对该 Peer 记一次失败
  RootMismatch,
  MalformedProgress,
  ProgressMismatch,
  /// `open` 找到的文件长度与声明的不符：要么不是同一个文件，要么被别的程序改过
  LengthMismatch {
    expected: u64,
    got: u64,
  },
  BlockSizeOutOfRange(u32),
  BlockSizeNotPowerOfTwo(u32),
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
      StoreError::RootMismatch => f.write_str("proof or data does not match the root"),
      StoreError::MalformedProgress => f.write_str("progress file is malformed"),
      StoreError::ProgressMismatch => f.write_str("progress file belongs to a different file"),
      StoreError::LengthMismatch { expected, got } => {
        write!(f, "file is {got} bytes, expected {expected}")
      }
      StoreError::BlockSizeOutOfRange(s) => write!(f, "block size out of range: {s}"),
      StoreError::BlockSizeNotPowerOfTwo(s) => write!(f, "block size is not a power of two: {s}"),
    }
  }
}

impl std::error::Error for StoreError {}

impl From<std::io::Error> for StoreError {
  fn from(e: std::io::Error) -> Self {
    StoreError::Io(e.to_string())
  }
}

#[derive(Debug)]
pub struct FileStore {
  path: PathBuf,
  length: u64,
  block_size: u32,
  blocks: Vec<Block>,
}

/// spec 5.2 的 block 大小规则，store 与 seed 用同一套，免得两边判出来的不一样。
fn check_block_size(block_size: u32) -> Result<(), StoreError> {
  if !(MIN_BLOCK_SIZE..=MAX_BLOCK_SIZE).contains(&block_size) {
    return Err(StoreError::BlockSizeOutOfRange(block_size));
  }
  if !block_size.is_power_of_two() {
    return Err(StoreError::BlockSizeNotPowerOfTwo(block_size));
  }
  Ok(())
}

fn blocks_of(length: u64, block_size: u32) -> Vec<Block> {
  (0..length.div_ceil(block_size as u64))
    .map(|i| {
      let start = i * block_size as u64;
      Block::new((length - start).min(block_size as u64) as usize)
    })
    .collect()
}

impl FileStore {
  /// 新建：已存在的同名文件会被清空。要接着上次的进度下载，用 `open`。
  pub fn create(path: impl AsRef<Path>, length: u64, block_size: u32) -> Result<Self, StoreError> {
    let path = path.as_ref().to_path_buf();
    check_block_size(block_size)?;
    let f = FsFile::create(&path)?;
    if length > 0 {
      f.set_len(length)?;
    }
    Ok(FileStore {
      path,
      length,
      block_size,
      blocks: blocks_of(length, block_size),
    })
  }

  /// 打开一个已经存在的文件，一个字节都不动——恢复进度必须走这里。
  /// `create` 会把它清零，那样位图还标着 Partial、字节却没了，是静默丢数据。
  pub fn open(path: impl AsRef<Path>, length: u64, block_size: u32) -> Result<Self, StoreError> {
    let path = path.as_ref().to_path_buf();
    check_block_size(block_size)?;
    let got = std::fs::metadata(&path)?.len();
    if got != length {
      return Err(StoreError::LengthMismatch {
        expected: length,
        got,
      });
    }
    Ok(FileStore {
      path,
      length,
      block_size,
      blocks: blocks_of(length, block_size),
    })
  }

  pub fn block_count(&self) -> usize {
    self.blocks.len()
  }

  pub fn block(&self, index: usize) -> Result<&Block, StoreError> {
    self
      .blocks
      .get(index)
      .ok_or(StoreError::BlockOutOfRange(index))
  }

  pub fn block_len(&self, index: usize) -> Result<usize, StoreError> {
    Ok(self.block(index)?.len())
  }

  pub fn piece_count(&self, index: usize) -> Result<usize, StoreError> {
    Ok(self.block(index)?.piece_count())
  }

  pub fn state(&self, index: usize) -> Result<BlockState, StoreError> {
    Ok(self.block(index)?.state())
  }

  /// Write one piece at its final offset and advance the block's state.
  pub fn write_piece(
    &mut self,
    block: usize,
    piece: usize,
    data: &[u8],
  ) -> Result<BlockState, StoreError> {
    let b = self
      .blocks
      .get_mut(block)
      .ok_or(StoreError::BlockOutOfRange(block))?;
    if piece >= b.piece_count() {
      return Err(StoreError::PieceOutOfRange(piece));
    }
    let expected = b.piece_len(piece);
    if data.len() != expected {
      return Err(StoreError::WrongDataLength {
        expected,
        got: data.len(),
      });
    }

    let offset = block as u64 * self.block_size as u64 + piece as u64 * PIECE_SIZE as u64;
    {
      let mut f = FsFile::options().write(true).open(&self.path)?;
      f.seek(SeekFrom::Start(offset))?;
      f.write_all(data)?;
    }
    Ok(b.mark_piece(piece))
  }

  /// Read a block back and check it against the tree root. On failure the block
  /// goes back to `Partial` with its bitmap cleared and `RootMismatch` comes
  /// back — spec 4.2 wants the caller to count a failure against whoever sent
  /// the data, which it cannot do if failure looks like success.
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
      self.blocks[block].mark_verified();
      Ok(BlockState::Verified)
    } else {
      // 字节已经不可信，位图清掉重来
      self.blocks[block].reset();
      Err(StoreError::RootMismatch)
    }
  }

  pub fn read_block(&self, block: usize) -> Result<Vec<u8>, StoreError> {
    let len = self.block(block)?.len();
    let offset = block as u64 * self.block_size as u64;
    let mut buf = vec![0u8; len];
    let mut f = FsFile::open(&self.path)?;
    f.seek(SeekFrom::Start(offset))?;
    f.read_exact(&mut buf)?;
    Ok(buf)
  }

  /// Bitfield for the wire: only verified blocks are set.
  pub fn verified_bitfield(&self) -> Vec<u8> {
    let mut out = vec![0u8; self.block_count().div_ceil(8)];
    for (i, b) in self.blocks.iter().enumerate() {
      if b.state() == BlockState::Verified {
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
    let states: Vec<BValue> = self
      .blocks
      .iter()
      .map(|b| BValue::Int(b.state() as i64))
      .collect();
    let maps: Vec<BValue> = self
      .blocks
      .iter()
      .map(|b| BValue::Bytes(b.bitmap_bytes()))
      .collect();

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
      // 只认 0..=3。写别的说明进度文件坏了，按 Verified 收下会让没校验过的
      // 数据被当作可信——宁可整个拒绝。
      self.blocks[i].state = match n {
        0 => BlockState::Missing,
        1 => BlockState::Partial,
        2 => BlockState::Downloaded,
        3 => BlockState::Verified,
        _ => return Err(StoreError::MalformedProgress),
      };
    }
    for (i, m) in maps.iter().enumerate() {
      let bytes = match m {
        BValue::Bytes(b) => b,
        _ => return Err(StoreError::MalformedProgress),
      };
      if !self.blocks[i].set_bitmap_bytes(bytes) {
        return Err(StoreError::MalformedProgress);
      }
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
    assert_eq!(store.block_len(0).unwrap(), 256 * 1024);
    assert_eq!(store.piece_count(0).unwrap(), 256);
    // 700_000 - 2*262_144 = 175_712
    assert_eq!(store.block_len(2).unwrap(), 175_712);
    assert_eq!(store.piece_count(2).unwrap(), 175_712 / PIECE_SIZE + 1);
    let last = store.piece_count(2).unwrap() - 1;
    assert_eq!(
      store.block(2).unwrap().piece_len(last),
      175_712 % PIECE_SIZE
    );
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
      assert_eq!(
        store.write_piece(0, p, &chunk).unwrap(),
        BlockState::Partial
      );
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
  fn piece_presence_is_tracked_per_block() {
    let dir = TempDir::new().unwrap();
    let mut store =
      FileStore::create(dir.path().join("f"), 4 * PIECE_SIZE as u64, 256 * 1024).unwrap();
    assert!(!store.block(0).unwrap().piece_present(0));
    store.write_piece(0, 0, &[1u8; PIECE_SIZE]).unwrap();
    assert!(store.block(0).unwrap().piece_present(0));
    assert!(!store.block(0).unwrap().piece_present(1));
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
    // wrong root on purpose：失败必须看得见，否则没法对该 Peer 记一次失败
    assert_eq!(
      store.verify(0, &proof, &[0xff; 32]),
      Err(StoreError::RootMismatch)
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
    for p in 0..store.piece_count(0).unwrap() {
      let start = p * PIECE_SIZE;
      let chunk = blocks[0][start..start + PIECE_SIZE].to_vec();
      store.write_piece(0, p, &chunk).unwrap();
    }
    for p in 0..store.piece_count(1).unwrap() {
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
    let length = 4 * PIECE_SIZE as u64;
    let mut store = FileStore::create(&path, length, block_size).unwrap();
    store.write_piece(0, 0, &[9u8; PIECE_SIZE]).unwrap();
    store.save_progress().unwrap();

    let mut reopened = FileStore::open(&path, length, block_size).unwrap();
    reopened.load_progress().unwrap();
    assert_eq!(reopened.state(0).unwrap(), BlockState::Partial);
    assert!(reopened.block(0).unwrap().piece_present(0));
    assert!(!reopened.block(0).unwrap().piece_present(1));
    // 字节必须还在。用 create 恢复会把文件清零，而位图仍标着已下载
    assert_eq!(
      &std::fs::read(&path).unwrap()[..PIECE_SIZE],
      &[9u8; PIECE_SIZE][..]
    );
  }

  #[test]
  fn open_keeps_the_bytes_where_create_clears_them() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("f");
    let block_size = 256 * 1024u32;
    let length = 4 * PIECE_SIZE as u64;
    let mut store = FileStore::create(&path, length, block_size).unwrap();
    store.write_piece(0, 0, &[7u8; PIECE_SIZE]).unwrap();

    let kept = FileStore::open(&path, length, block_size).unwrap();
    assert_eq!(
      &kept.read_block(0).unwrap()[..PIECE_SIZE],
      &[7u8; PIECE_SIZE][..]
    );

    FileStore::create(&path, length, block_size).unwrap();
    assert_eq!(
      &std::fs::read(&path).unwrap()[..PIECE_SIZE],
      &[0u8; PIECE_SIZE][..]
    );
  }

  #[test]
  fn open_rejects_a_missing_file_and_a_wrong_length() {
    let dir = TempDir::new().unwrap();
    let missing = dir.path().join("nope");
    assert!(matches!(
      FileStore::open(&missing, 4 * PIECE_SIZE as u64, 256 * 1024),
      Err(StoreError::Io(_))
    ));

    let path = dir.path().join("f");
    FileStore::create(&path, 4 * PIECE_SIZE as u64, 256 * 1024).unwrap();
    match FileStore::open(&path, 8 * PIECE_SIZE as u64, 256 * 1024).unwrap_err() {
      StoreError::LengthMismatch { expected, got } => {
        assert_eq!(expected, 8 * PIECE_SIZE as u64);
        assert_eq!(got, 4 * PIECE_SIZE as u64);
      }
      other => panic!("expected LengthMismatch, got {other}"),
    }
  }

  #[test]
  fn create_rejects_a_block_size_seed_would_reject() {
    let dir = TempDir::new().unwrap();
    assert!(matches!(
      FileStore::create(dir.path().join("f"), 1024, 1000),
      Err(StoreError::BlockSizeOutOfRange(1000))
    ));
    // 模式里不能写算术，提一个常量出来
    const ODD: u32 = 3 * 256 * 1024;
    assert!(matches!(
      FileStore::create(dir.path().join("g"), 1024, ODD),
      Err(StoreError::BlockSizeNotPowerOfTwo(ODD))
    ));
  }

  #[test]
  fn progress_with_an_unknown_block_state_is_rejected() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("f");
    let block_size = 256 * 1024u32;
    let length = 4 * PIECE_SIZE as u64;
    let store = FileStore::create(&path, length, block_size).unwrap();
    store.save_progress().unwrap();

    // 手工把状态写成 9：旧实现落在 `_ => Verified` 上，等于没校验就信任
    let mut d = match bencode::decode(&std::fs::read(store.progress_path()).unwrap()).unwrap() {
      BValue::Dict(d) => d,
      _ => unreachable!(),
    };
    d.insert(b"states".to_vec(), BValue::List(vec![BValue::Int(9)]));
    std::fs::write(store.progress_path(), BValue::Dict(d).encode()).unwrap();

    let mut reopened = FileStore::open(&path, length, block_size).unwrap();
    assert_eq!(reopened.load_progress(), Err(StoreError::MalformedProgress));
  }

  #[test]
  fn rewriting_a_verified_block_drops_it_back_to_downloaded() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("f");
    let block_size = 256 * 1024u32;
    let mut store = FileStore::create(&path, 2 * PIECE_SIZE as u64, block_size).unwrap();
    let data = vec![5u8; 2 * PIECE_SIZE];
    for p in 0..2 {
      let start = p * PIECE_SIZE;
      store
        .write_piece(0, p, &data[start..start + PIECE_SIZE])
        .unwrap();
    }
    let leaf = leaf_hash(&data);
    let proof = merkle::prove(&[leaf], 0);
    let root = merkle::root(&[leaf]);
    assert_eq!(
      store.verify(0, &proof, &root).unwrap(),
      BlockState::Verified
    );

    // 校验通过之后又被写入：字节已经变了，不能再对外声称 verified
    assert_eq!(
      store.write_piece(0, 0, &[6u8; PIECE_SIZE]).unwrap(),
      BlockState::Downloaded
    );
    assert_eq!(store.state(0).unwrap(), BlockState::Downloaded);
    assert!(store.verified_bitfield().iter().all(|b| *b == 0));
  }

  #[test]
  fn out_of_range_queries_answer_instead_of_panicking() {
    let dir = TempDir::new().unwrap();
    let store = FileStore::create(dir.path().join("f"), 4 * PIECE_SIZE as u64, 256 * 1024).unwrap();
    let block = store.block(0).unwrap();
    assert_eq!(block.piece_len(block.piece_count()), 0);
    assert!(!block.piece_present(block.piece_count()));
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
