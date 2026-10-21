use std::{fmt, io, time::Duration};

/// Errors never include request URLs, which may contain signed grants.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
  Invalid(&'static str),
  Http(reqwest::Error),
  Timeout,
  Status {
    code: u16,
    retry_after: Option<Duration>,
  },
  Protocol(&'static str),
  RangeUnsupported,
  SourceChanged,
  Integrity,
  Busy,
  Checkpoint(&'static str),
  Io(io::Error),
  Cancelled,
  Task(tokio::task::JoinError),
}
impl Error {
  pub(crate) fn http(error: reqwest::Error) -> Self {
    if error.is_timeout() {
      Self::Timeout
    } else {
      Self::Http(error.without_url())
    }
  }
  pub(crate) fn retryable(&self) -> bool {
    matches!(
      self,
      Self::Http(_)
        | Self::Timeout
        | Self::Status {
          code: 408 | 429 | 500..=599,
          ..
        }
    )
  }
}
impl fmt::Display for Error {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::Invalid(m) => write!(f, "invalid download configuration: {m}"),
      Self::Http(e) => write!(f, "HTTP request failed: {e}"),
      Self::Timeout => f.write_str("HTTP request deadline exceeded"),
      Self::Status { code, .. } => write!(f, "HTTP status {code}"),
      Self::Protocol(m) => write!(f, "invalid HTTP response: {m}"),
      Self::RangeUnsupported => f.write_str("server ignored the requested byte range"),
      Self::SourceChanged => f.write_str("source ETag changed"),
      Self::Integrity => f.write_str("file checksum does not match expected content"),
      Self::Busy => f.write_str("download destination is locked by another task"),
      Self::Checkpoint(m) => write!(f, "invalid download checkpoint: {m}"),
      Self::Io(e) => write!(f, "download storage failed: {e}"),
      Self::Cancelled => f.write_str("download cancelled"),
      Self::Task(e) => write!(f, "download task failed: {e}"),
    }
  }
}
impl std::error::Error for Error {
  fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
    match self {
      Self::Http(e) => Some(e),
      Self::Io(e) => Some(e),
      Self::Task(e) => Some(e),
      _ => None,
    }
  }
}
impl From<io::Error> for Error {
  fn from(e: io::Error) -> Self {
    Self::Io(e)
  }
}
impl From<tokio::task::JoinError> for Error {
  fn from(e: tokio::task::JoinError) -> Self {
    Self::Task(e)
  }
}
