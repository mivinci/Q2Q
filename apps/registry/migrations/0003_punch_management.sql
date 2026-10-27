-- Management intent outlives heartbeat leases and is never written by reports.
CREATE TABLE punch_policy (
  node_id TEXT PRIMARY KEY NOT NULL,
  enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),
  revision INTEGER NOT NULL CHECK(revision BETWEEN 1 AND 9007199254740991),
  updated_at INTEGER NOT NULL
);
CREATE TABLE punch_operation (
  request_id TEXT PRIMARY KEY NOT NULL,
  actor_id TEXT NOT NULL,
  actor_email TEXT NOT NULL,
  node_id TEXT NOT NULL,
  reason TEXT NOT NULL,
  fingerprint TEXT NOT NULL,
  before_enabled INTEGER NOT NULL CHECK(before_enabled IN (0,1)),
  before_revision INTEGER NOT NULL,
  after_enabled INTEGER NOT NULL CHECK(after_enabled IN (0,1)),
  after_revision INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  applied_at INTEGER
);
CREATE INDEX punch_operation_node ON punch_operation(node_id, created_at DESC);
