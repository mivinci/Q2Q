//! One block of a file: its state, and which of its pieces have landed.
//!
//! Piece-level bookkeeping only — no file I/O lives here. The state machine is
//! kept away from the code that writes bytes so its invariants cannot be bent
//! by accident, which is how a bitmap ended up disagreeing with the file once
//! before.

/// The fixed piece size (spec 4.2). A block is cut into these, and each one
/// travels as its own data-channel message, so a loss only costs that piece.
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
  pub(crate) state: BlockState,
  /// Byte length; only the last block is shorter than the store's block size.
  pub(crate) len: usize,
  /// One bit per piece, held as u64 words.
  pub(crate) bitmap: Vec<u64>,
}

impl Block {
  pub(crate) fn new(len: usize) -> Self {
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

  pub(crate) fn set_piece(&mut self, piece: usize, present: bool) {
    let (word, bit) = (piece / 64, piece % 64);
    if present {
      self.bitmap[word] |= 1 << bit;
    } else {
      self.bitmap[word] &= !(1 << bit);
    }
  }

  pub(crate) fn all_pieces_present(&self) -> bool {
    (0..self.piece_count()).all(|p| self.piece_present(p))
  }

  /// Record a piece and advance the state. Returns the state afterwards.
  pub(crate) fn mark_piece(&mut self, piece: usize) -> BlockState {
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

  pub(crate) fn mark_verified(&mut self) {
    self.state = BlockState::Verified;
  }

  /// Drop every piece. Called when verification fails: a root mismatch usually
  /// means the bytes themselves came from a bad peer, so keeping them would
  /// only fail again.
  pub(crate) fn reset(&mut self) {
    self.bitmap.fill(0);
    self.state = BlockState::Partial;
  }

  pub(crate) fn bitmap_bytes(&self) -> Vec<u8> {
    self.bitmap.iter().flat_map(|w| w.to_le_bytes()).collect()
  }

  pub(crate) fn set_bitmap_bytes(&mut self, bytes: &[u8]) -> bool {
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

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn pieces_cover_the_whole_block() {
    let b = Block::new(3 * PIECE_SIZE + 7);
    assert_eq!(b.piece_count(), 4);
    assert_eq!(b.piece_len(0), PIECE_SIZE);
    // 只有最后一条可以短
    assert_eq!(b.piece_len(3), 7);
  }

  #[test]
  fn marking_pieces_advances_the_state() {
    let mut b = Block::new(2 * PIECE_SIZE);
    assert_eq!(b.state(), BlockState::Missing);
    assert_eq!(b.mark_piece(0), BlockState::Partial);
    assert_eq!(b.mark_piece(1), BlockState::Downloaded);
  }

  #[test]
  fn a_verified_block_falls_back_when_written_again() {
    let mut b = Block::new(2 * PIECE_SIZE);
    b.mark_piece(0);
    b.mark_piece(1);
    b.mark_verified();
    assert_eq!(b.state(), BlockState::Verified);
    // 字节变了：不能再对外声称 verified，落回"数据齐了但得重新校验"
    assert_eq!(b.mark_piece(0), BlockState::Downloaded);
  }

  #[test]
  fn reset_drops_every_piece() {
    let mut b = Block::new(2 * PIECE_SIZE);
    b.mark_piece(0);
    b.mark_verified();
    b.reset();
    assert_eq!(b.state(), BlockState::Partial);
    assert!(!b.piece_present(0));
  }

  #[test]
  fn out_of_range_queries_answer_instead_of_panicking() {
    let b = Block::new(2 * PIECE_SIZE);
    assert_eq!(b.piece_len(b.piece_count()), 0);
    assert!(!b.piece_present(b.piece_count()));
  }

  #[test]
  fn bitmap_round_trips_through_bytes() {
    let mut b = Block::new(130 * PIECE_SIZE);
    b.mark_piece(0);
    b.mark_piece(65);
    let bytes = b.bitmap_bytes();

    let mut other = Block::new(130 * PIECE_SIZE);
    assert!(other.set_bitmap_bytes(&bytes));
    assert!(other.piece_present(0));
    assert!(other.piece_present(65));
    assert!(!other.piece_present(1));
    // 长度不对要拒绝，而不是截断着收下
    assert!(!other.set_bitmap_bytes(&bytes[..4]));
  }
}
