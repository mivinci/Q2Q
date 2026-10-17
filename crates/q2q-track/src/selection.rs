//! Candidate selection over ordered storage, shared by memory and Redis backends.
use crate::state::{Candidate, InfoHash, StateError, TrackState, UnixSeconds};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use q2q_core::identity::PeerId;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

const BATCH: usize = 64;
const SCAN_BUDGET: usize = 512;
pub const MAX_CURSOR_BYTES: usize = 1024;

pub struct Query<'a> {
  pub info_hash: &'a InfoHash,
  pub requester: &'a PeerId,
  pub limit: usize,
  pub cursor: Option<&'a str>,
  pub punch_ids: &'a HashSet<String>,
  pub now: UnixSeconds,
}
pub struct Selection {
  pub peers: Vec<Candidate>,
  pub next_cursor: Option<String>,
}
#[derive(Debug)]
pub enum SelectionError {
  InvalidCursor,
  InvalidLimit,
  State(StateError),
}
impl From<StateError> for SelectionError {
  fn from(error: StateError) -> Self {
    Self::State(error)
  }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
  hash: InfoHash,
  requester: String,
  pivot: Option<String>,
  after: Option<String>,
  wrapped: bool,
}
impl Cursor {
  fn encode(&self) -> String {
    format!(
      "v1.{}",
      B64.encode(serde_json::to_vec(self).expect("cursor contains only strings and integers"))
    )
  }
  fn valid_id(id: &str) -> bool {
    PeerId::unchecked(id).checksum_is_valid()
  }
  fn decode(raw: &str, query: &Query<'_>) -> Result<Self, SelectionError> {
    let invalid = || SelectionError::InvalidCursor;
    if raw.len() > MAX_CURSOR_BYTES {
      return Err(invalid());
    }
    if let Some(encoded) = raw.strip_prefix("v1.") {
      let bytes = B64.decode(encoded).map_err(|_| invalid())?;
      let cursor: Self = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
      if cursor.hash != *query.info_hash
        || cursor.requester != query.requester.as_str()
        || cursor
          .pivot
          .as_deref()
          .is_some_and(|id| !Self::valid_id(id))
        || cursor
          .after
          .as_deref()
          .is_some_and(|id| !Self::valid_id(id))
        || (cursor.wrapped && cursor.pivot.is_none())
        || (!cursor.wrapped && cursor.pivot.is_some() && cursor.after.is_none())
      {
        return Err(invalid());
      }
      if let (Some(pivot), Some(after)) = (&cursor.pivot, &cursor.after)
        && ((!cursor.wrapped && after < pivot) || (cursor.wrapped && after > pivot))
      {
        return Err(invalid());
      }
      Ok(cursor)
    } else if Self::valid_id(raw) {
      // Legacy cursors continue lexically without rotation or wraparound.
      Ok(Self {
        hash: *query.info_hash,
        requester: query.requester.to_string(),
        pivot: None,
        after: Some(raw.into()),
        wrapped: false,
      })
    } else {
      Err(invalid())
    }
  }
}
impl Query<'_> {
  /// `start` must be a valid fresh random Peer ID supplied by the host for initial pages.
  /// Continuations keep their own start, so different Track instances can serve
  /// the same walk. This is live pagination, not a snapshot or uniform sampling.
  pub async fn select(
    &self,
    state: &impl TrackState,
    start: PeerId,
  ) -> Result<Selection, SelectionError> {
    if !(1..=crate::wire::MAX_QUERY_LIMIT).contains(&self.limit) {
      return Err(SelectionError::InvalidLimit);
    }
    if self.cursor.is_none() && !start.checksum_is_valid() {
      return Err(SelectionError::InvalidCursor);
    }
    let mut cursor = match self.cursor {
      Some(raw) => Cursor::decode(raw, self)?,
      None => Cursor {
        hash: *self.info_hash,
        requester: self.requester.to_string(),
        pivot: Some(start.to_string()),
        after: Some(start.to_string()),
        wrapped: false,
      },
    };
    let mut peers = Vec::with_capacity(self.limit);
    let mut scanned = 0;
    loop {
      if scanned == SCAN_BUDGET {
        return Ok(Selection {
          peers,
          next_cursor: Some(cursor.encode()),
        });
      }
      let count = BATCH.min(SCAN_BUDGET - scanned);
      let after = cursor.after.as_ref().map(PeerId::unchecked);
      let batch = state
        .query(self.info_hash, count, after.as_ref(), self.now)
        .await?;
      let end = batch.len() < count;
      for candidate in batch {
        if cursor.wrapped
          && cursor
            .pivot
            .as_ref()
            .is_some_and(|pivot| candidate.peer_id.as_str() > pivot.as_str())
        {
          return Ok(Selection {
            peers,
            next_cursor: None,
          });
        }
        // Save progress before the lookahead candidate, including filtered
        // records, so a continuation neither skips it nor rescans a bad prefix.
        let before = cursor.clone();
        scanned += 1;
        cursor.after = Some(candidate.peer_id.to_string());
        if candidate.peer_id == *self.requester || !self.punch_ids.contains(&candidate.punch_id) {
          continue;
        }
        if peers.len() == self.limit {
          return Ok(Selection {
            peers,
            next_cursor: Some(before.encode()),
          });
        }
        peers.push(candidate);
      }
      if end {
        if cursor.pivot.is_some() && !cursor.wrapped {
          cursor.wrapped = true;
          cursor.after = None;
        } else {
          return Ok(Selection {
            peers,
            next_cursor: None,
          });
        }
      }
    }
  }
}
