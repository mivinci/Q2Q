CREATE TABLE storage_connection (
  id TEXT PRIMARY KEY NOT NULL,
  projectId TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
  provider TEXT NOT NULL CHECK(provider = 'platform-r2'),
  createdAt INTEGER NOT NULL,
  UNIQUE(projectId, provider)
);
