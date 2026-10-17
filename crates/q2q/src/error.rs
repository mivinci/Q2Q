//! SDK transfer errors, independent of task handles and transport implementations.
use crate::control::ControlError;
use crate::source_file::PrepareError;
use q2q_core::file::StoreError;
use q2q_core::peer_wire::WireError;

#[derive(Debug)]
pub enum TransferError {
  Control(ControlError),
  Prepare(PrepareError),
  Store(StoreError),
  Wire(WireError),
  Io(std::io::Error),
  Task(tokio::task::JoinError),
  Protocol(String),
  Checkpoint(String),
  Cdn(String),
  Closed,
  Timeout,
  Rejected(u32),
  NoPeers,
  AlreadyPublishing,
}

impl std::fmt::Display for TransferError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      Self::Control(e) => e.fmt(f),
      Self::Prepare(e) => e.fmt(f),
      Self::Store(e) => e.fmt(f),
      Self::Wire(e) => e.fmt(f),
      Self::Io(e) => e.fmt(f),
      Self::Task(e) => e.fmt(f),
      Self::Protocol(e) => write!(f, "peer transfer protocol: {e}"),
      Self::Cdn(e) => write!(f, "CDN fallback: {e}"),
      Self::Checkpoint(e) => write!(f, "download checkpoint: {e}"),
      Self::Closed => f.write_str("peer transfer channel closed"),
      Self::Timeout => f.write_str("peer transfer timed out"),
      Self::Rejected(ms) => write!(f, "peer rejected request; retry after {ms} ms"),
      Self::NoPeers => f.write_str("no usable peer is available"),
      Self::AlreadyPublishing => f.write_str("this content already has an active publication"),
    }
  }
}
impl std::error::Error for TransferError {
  fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
    match self {
      Self::Control(error) => Some(error),
      Self::Prepare(error) => Some(error),
      Self::Store(error) => Some(error),
      Self::Wire(error) => Some(error),
      Self::Io(error) => Some(error),
      Self::Task(error) => Some(error),
      _ => None,
    }
  }
}
macro_rules! convert {
  ($ty:ty, $variant:ident) => {
    impl From<$ty> for TransferError {
      fn from(e: $ty) -> Self {
        Self::$variant(e)
      }
    }
  };
}
convert!(ControlError, Control);
convert!(PrepareError, Prepare);
convert!(StoreError, Store);
convert!(WireError, Wire);
convert!(std::io::Error, Io);
convert!(tokio::task::JoinError, Task);

impl TransferError {
  pub(crate) fn peer_retryable(&self) -> bool {
    matches!(
      self,
      Self::Control(_)
        | Self::Wire(_)
        | Self::Protocol(_)
        | Self::Closed
        | Self::Timeout
        | Self::Rejected(_)
        | Self::Store(StoreError::RootMismatch)
    )
  }
}
