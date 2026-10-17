CREATE TABLE upload_task (
  id TEXT PRIMARY KEY NOT NULL,
  projectId TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
  connectionId TEXT NOT NULL REFERENCES storage_connection(id),
  createdBy TEXT NOT NULL REFERENCES user(id),
  name TEXT NOT NULL,
  size INTEGER NOT NULL CHECK(size > 0),
  objectKey TEXT NOT NULL UNIQUE,
  multipartId TEXT,
  partSize INTEGER NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('initializing', 'uploading', 'completing', 'uploaded', 'cancelled', 'failed')),
  partsJson TEXT,
  objectVersion TEXT,
  objectEtag TEXT,
  cleanupPending INTEGER NOT NULL DEFAULT 0,
  createdAt INTEGER NOT NULL,
  updatedAt INTEGER NOT NULL,
  expiresAt INTEGER NOT NULL
);
CREATE INDEX upload_task_project_created_idx ON upload_task(projectId, createdAt DESC);
