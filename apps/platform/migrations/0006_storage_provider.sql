-- Preserve connection IDs and upload references while broadening the provider to S3 compatibility.
PRAGMA defer_foreign_keys = ON;
CREATE TABLE storage_connection_new (
  id TEXT PRIMARY KEY NOT NULL,
  projectId TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
  provider TEXT NOT NULL CHECK(provider = 'platform-s3'),
  createdAt INTEGER NOT NULL,
  UNIQUE(projectId, provider)
);
INSERT INTO storage_connection_new (id, projectId, provider, createdAt)
  SELECT id, projectId, 'platform-s3', createdAt FROM storage_connection;
DROP TABLE storage_connection;
ALTER TABLE storage_connection_new RENAME TO storage_connection;
