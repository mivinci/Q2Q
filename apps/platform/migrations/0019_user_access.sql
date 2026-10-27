-- Defaults preserve existing accounts and sessions until an administrator acts.
ALTER TABLE user ADD COLUMN disabledAt INTEGER CHECK(disabledAt IS NULL OR (typeof(disabledAt) = 'integer' AND disabledAt >= 0));
ALTER TABLE user ADD COLUMN disabledReason TEXT CHECK(
  (disabledAt IS NULL AND disabledReason IS NULL) OR
  (disabledAt IS NOT NULL AND disabledReason IS NOT NULL AND length(trim(disabledReason)) BETWEEN 1 AND 1000)
);
ALTER TABLE user ADD COLUMN accessRevision INTEGER NOT NULL DEFAULT 1
  CHECK(typeof(accessRevision) = 'integer' AND accessRevision BETWEEN 1 AND 9007199254740991);
ALTER TABLE user ADD COLUMN authVersion INTEGER NOT NULL DEFAULT 1
  CHECK(typeof(authVersion) = 'integer' AND authVersion BETWEEN 1 AND 9007199254740991);
ALTER TABLE session ADD COLUMN authVersion INTEGER NOT NULL DEFAULT 1
  CHECK(typeof(authVersion) = 'integer' AND authVersion BETWEEN 1 AND 9007199254740991);

CREATE TRIGGER user_access_revision AFTER UPDATE OF disabledAt, disabledReason, authVersion ON user
  WHEN NEW.accessRevision = OLD.accessRevision
  BEGIN
    UPDATE user SET accessRevision = OLD.accessRevision + 1 WHERE id = NEW.id;
  END;
-- The application hook stamps the current version. These checks close the race
-- between that read and the actual session insert/refresh.
CREATE TRIGGER session_access_insert BEFORE INSERT ON session
  WHEN NOT EXISTS (SELECT 1 FROM user WHERE id = NEW.userId AND disabledAt IS NULL AND authVersion = NEW.authVersion)
  BEGIN SELECT RAISE(ABORT, 'account session unavailable'); END;
CREATE TRIGGER session_access_update BEFORE UPDATE ON session
  WHEN NOT EXISTS (SELECT 1 FROM user WHERE id = NEW.userId AND disabledAt IS NULL AND authVersion = NEW.authVersion)
  BEGIN SELECT RAISE(ABORT, 'account session unavailable'); END;

DROP TRIGGER admin_operation_applied;
ALTER TABLE admin_operation RENAME TO admin_operation_previous;
CREATE TABLE admin_operation (
  requestId TEXT PRIMARY KEY NOT NULL,
  actorId TEXT NOT NULL,
  actorEmail TEXT NOT NULL,
  action TEXT NOT NULL CHECK(action IN ('plan.update', 'subscription.set', 'user.disable', 'user.restore', 'user.sessions.revoke')),
  targetId TEXT NOT NULL,
  reason TEXT NOT NULL CHECK(length(trim(reason)) BETWEEN 1 AND 1000),
  fingerprint TEXT NOT NULL CHECK(length(fingerprint) = 64),
  beforeJson TEXT NOT NULL CHECK(json_valid(beforeJson)),
  afterJson TEXT NOT NULL CHECK(json_valid(afterJson)),
  revision INTEGER NOT NULL CHECK(revision BETWEEN 1 AND 9007199254740991),
  createdAt INTEGER NOT NULL,
  appliedAt INTEGER
);
INSERT INTO admin_operation SELECT * FROM admin_operation_previous;
DROP TABLE admin_operation_previous;
CREATE INDEX admin_operation_target ON admin_operation(action, targetId, createdAt DESC, requestId);
CREATE INDEX admin_operation_time ON admin_operation(createdAt DESC, requestId);

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
      WHEN NEW.action IN ('user.disable', 'user.restore', 'user.sessions.revoke') AND NOT EXISTS (
        SELECT 1 FROM user WHERE id = NEW.targetId AND accessRevision = NEW.revision
      ) THEN RAISE(ABORT, 'user operation was not applied')
      WHEN NEW.action IN ('user.disable', 'user.restore', 'user.sessions.revoke') AND EXISTS (
        SELECT 1 FROM session WHERE userId = NEW.targetId
      ) THEN RAISE(ABORT, 'user sessions were not removed')
    END;
  END;
