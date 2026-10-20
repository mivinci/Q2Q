//! Punch discovery is deployment state; request handlers never perform network I/O.
use parking_lot::RwLock;
use q2q_primitives::identity::PeerId;
use q2q_track::state::PunchAssignment;
use std::{collections::HashMap, sync::Arc};

#[derive(Debug, Clone, Copy)]
pub enum DirectoryError {
  Unavailable,
}

pub struct DirectorySnapshot {
  pub nodes: HashMap<String, PunchAssignment>,
  pub assignable: Vec<String>,
  /// Registry identity changes require the Peer to announce again. Static mode
  /// preserves the existing address reload behaviour.
  pub strict_identity: bool,
}

pub trait PunchDirectory: Send + Sync {
  fn snapshot(&self) -> Result<DirectorySnapshot, DirectoryError>;

  fn assign(&self, _peer: &PeerId) -> Result<Option<PunchAssignment>, DirectoryError> {
    let snapshot = self.snapshot()?;
    Ok(
      snapshot
        .assignable
        .first()
        .and_then(|id| snapshot.nodes.get(id))
        .cloned(),
    )
  }

  fn get(&self, id: &str) -> Result<Option<PunchAssignment>, DirectoryError> {
    Ok(self.snapshot()?.nodes.remove(id))
  }

  fn requires_matching_identity(&self) -> bool {
    false
  }
}

pub struct StaticPunchDirectory(RwLock<Vec<PunchAssignment>>);

impl StaticPunchDirectory {
  pub fn new(list: Vec<PunchAssignment>) -> Arc<Self> {
    Arc::new(Self(RwLock::new(list)))
  }

  pub fn replace(&self, list: Vec<PunchAssignment>) {
    *self.0.write() = list;
  }
}

impl PunchDirectory for StaticPunchDirectory {
  fn get(&self, id: &str) -> Result<Option<PunchAssignment>, DirectoryError> {
    Ok(
      self
        .0
        .read()
        .iter()
        .find(|node| node.punch_id == id)
        .cloned(),
    )
  }

  fn snapshot(&self) -> Result<DirectorySnapshot, DirectoryError> {
    let list = self.0.read();
    Ok(DirectorySnapshot {
      nodes: list
        .iter()
        .map(|node| (node.punch_id.clone(), node.clone()))
        .collect(),
      assignable: list.iter().map(|node| node.punch_id.clone()).collect(),
      strict_identity: false,
    })
  }
}
