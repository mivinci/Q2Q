CREATE TABLE seed_task (
  fileId TEXT PRIMARY KEY NOT NULL REFERENCES file_record(id),
  uploadId TEXT NOT NULL UNIQUE REFERENCES upload_task(id),
  status TEXT NOT NULL CHECK(status IN ('pending', 'running', 'ready', 'failed')),
  attempt INTEGER NOT NULL DEFAULT 0,
  leaseToken TEXT,
  leaseUntil INTEGER,
  processedBytes INTEGER NOT NULL DEFAULT 0,
  seedKey TEXT,
  proofKey TEXT,
  infoHash TEXT,
  errorCode TEXT,
  createdAt INTEGER NOT NULL,
  updatedAt INTEGER NOT NULL
);
CREATE INDEX seed_task_status_idx ON seed_task(status, leaseUntil);
