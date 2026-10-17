//! Shared token bucket with FIFO turns for ready upload connections.
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::Instant;

pub(crate) struct UploadRate {
  rate: u64,
  capacity: f64,
  state: Mutex<State>,
}
struct State {
  tokens: f64,
  updated: Instant,
}
impl UploadRate {
  pub(crate) fn new(rate: u64) -> Self {
    assert!(rate > 0);
    // Permit 100 ms of burst traffic, or one full encoded piece at low rates.
    let capacity = (rate as f64 / 10.0)
      .max((q2q_core::block::PIECE_SIZE + q2q_core::peer_wire::PIECE_HEADER) as f64);
    Self {
      rate,
      capacity,
      state: Mutex::new(State {
        tokens: capacity,
        updated: Instant::now(),
      }),
    }
  }
  pub(crate) async fn acquire(&self, bytes: usize) {
    assert!(bytes as f64 <= self.capacity);
    // Managed connections request one piece at a time. Tokio's FIFO mutex
    // gives each queued connection one turn, then it rejoins at the back.
    // Keep the turn while waiting for tokens, release it before network I/O.
    let mut state = self.state.lock().await;
    loop {
      let now = Instant::now();
      state.tokens = (state.tokens
        + now.duration_since(state.updated).as_secs_f64() * self.rate as f64)
        .min(self.capacity);
      state.updated = now;
      if state.tokens >= bytes as f64 {
        state.tokens -= bytes as f64;
        return;
      }
      let delay = Duration::from_secs_f64(
        ((bytes as f64 - state.tokens) / self.rate as f64).max(0.000_000_001),
      );
      // Cancellation drops the turn or queued mutex waiter. No tokens are
      // charged before the grant, and no future bandwidth debt is reserved.
      tokio::time::sleep(delay).await;
    }
  }
}

#[cfg(test)]
#[path = "../tests/upload_rate.rs"]
mod tests;
