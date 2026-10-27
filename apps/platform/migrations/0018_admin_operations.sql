-- Direct definition updates also invalidate stale editor previews.
CREATE TRIGGER plan_catalog_revision AFTER UPDATE ON plan_catalog
  WHEN NEW.revision = OLD.revision
  BEGIN
    UPDATE plan_catalog SET revision = OLD.revision + 1 WHERE id = NEW.id;
  END;

-- All subscription writers (including platform's simulation action) participate
-- in optimistic concurrency, even if they do not explicitly set revision.
ALTER TABLE project_subscription ADD COLUMN revision INTEGER NOT NULL DEFAULT 1
  CHECK(typeof(revision) = 'integer' AND revision BETWEEN 1 AND 9007199254740991);
CREATE TRIGGER project_subscription_revision AFTER UPDATE ON project_subscription
  WHEN NEW.revision = OLD.revision
  BEGIN
    UPDATE project_subscription SET revision = OLD.revision + 1 WHERE projectId = NEW.projectId;
  END;

-- Business changes and their audit records commit in the same D1 batch.
-- Request IDs are global; replay also requires the original actor and payload.
CREATE TABLE admin_operation (
  requestId TEXT PRIMARY KEY NOT NULL,
  actorId TEXT NOT NULL,
  actorEmail TEXT NOT NULL,
  action TEXT NOT NULL CHECK(action IN ('plan.update', 'subscription.set')),
  targetId TEXT NOT NULL,
  reason TEXT NOT NULL CHECK(length(trim(reason)) BETWEEN 1 AND 1000),
  fingerprint TEXT NOT NULL CHECK(length(fingerprint) = 64),
  beforeJson TEXT NOT NULL CHECK(json_valid(beforeJson)),
  afterJson TEXT NOT NULL CHECK(json_valid(afterJson)),
  revision INTEGER NOT NULL CHECK(revision BETWEEN 1 AND 9007199254740991),
  createdAt INTEGER NOT NULL,
  appliedAt INTEGER
);
CREATE INDEX admin_operation_target ON admin_operation(action, targetId, createdAt DESC, requestId);
CREATE INDEX admin_operation_time ON admin_operation(createdAt DESC, requestId);

-- A successful audit result must describe the revision actually committed.
CREATE TRIGGER admin_operation_applied BEFORE UPDATE OF appliedAt ON admin_operation
  WHEN OLD.appliedAt IS NULL AND NEW.appliedAt IS NOT NULL
  BEGIN
    SELECT CASE
      WHEN NEW.action = 'plan.update' AND NOT EXISTS (
        SELECT 1 FROM plan_catalog WHERE id = NEW.targetId AND revision = NEW.revision
      ) THEN RAISE(ABORT, 'plan operation was not applied')
      WHEN NEW.action = 'subscription.set' AND NOT EXISTS (
        SELECT 1 FROM project_subscription WHERE projectId = NEW.targetId AND revision = NEW.revision
      ) THEN RAISE(ABORT, 'subscription operation was not applied')
    END;
  END;
