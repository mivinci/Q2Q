CREATE TABLE file_record (
  id TEXT PRIMARY KEY NOT NULL,
  projectId TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
  createdBy TEXT NOT NULL REFERENCES user(id),
  name TEXT NOT NULL,
  size INTEGER NOT NULL CHECK(size > 0),
  status TEXT NOT NULL CHECK(status IN ('pending', 'uploaded', 'deleting', 'deleted')),
  createdAt INTEGER NOT NULL,
  updatedAt INTEGER NOT NULL
);
CREATE INDEX file_record_project_created_idx ON file_record(projectId, createdAt DESC);
-- Preserve existing task identities as file identities during this pre-release migration.
INSERT INTO file_record (id, projectId, createdBy, name, size, status, createdAt, updatedAt)
  SELECT id, projectId, createdBy, name, size,
    CASE WHEN status = 'uploaded' THEN 'uploaded' ELSE 'pending' END, createdAt, updatedAt FROM upload_task;
ALTER TABLE upload_task ADD COLUMN fileId TEXT REFERENCES file_record(id);
UPDATE upload_task SET fileId = id;
CREATE INDEX upload_task_file_idx ON upload_task(fileId);
CREATE UNIQUE INDEX upload_task_active_file_idx ON upload_task(fileId)
  WHERE status IN ('initializing', 'uploading', 'completing');
CREATE TRIGGER upload_task_require_file BEFORE INSERT ON upload_task
  WHEN NEW.fileId IS NULL BEGIN SELECT RAISE(ABORT, 'upload requires fileId'); END;
