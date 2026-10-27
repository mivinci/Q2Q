-- Local intent/result cache; Registry owns the authoritative policy and audit.
CREATE TABLE registry_request (
  requestId TEXT PRIMARY KEY NOT NULL,
  actorId TEXT NOT NULL,
  actorEmail TEXT NOT NULL,
  targetId TEXT NOT NULL,
  reason TEXT NOT NULL,
  fingerprint TEXT NOT NULL,
  payloadJson TEXT NOT NULL CHECK(json_valid(payloadJson)),
  status TEXT NOT NULL CHECK(status IN ('pending','applied','rejected')),
  resultJson TEXT CHECK(resultJson IS NULL OR json_valid(resultJson)),
  createdAt INTEGER NOT NULL,
  updatedAt INTEGER NOT NULL
);
CREATE INDEX registry_request_time ON registry_request(createdAt DESC,requestId);
CREATE TRIGGER registry_request_collision BEFORE INSERT ON registry_request
 WHEN EXISTS(SELECT 1 FROM admin_operation WHERE requestId=NEW.requestId)
 BEGIN SELECT RAISE(ABORT,'request_conflict'); END;
CREATE TRIGGER admin_operation_registry_collision BEFORE INSERT ON admin_operation
 WHEN EXISTS(SELECT 1 FROM registry_request WHERE requestId=NEW.requestId)
 BEGIN SELECT RAISE(ABORT,'request_conflict'); END;
