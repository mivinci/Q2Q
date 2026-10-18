CREATE TABLE project_network (
  projectId TEXT PRIMARY KEY NOT NULL REFERENCES project(id),
  mode TEXT NOT NULL CHECK(mode IN ('platform', 'custom')),
  trackUrl TEXT,
  updatedAt INTEGER NOT NULL,
  CHECK((mode = 'custom' AND trackUrl IS NOT NULL) OR (mode = 'platform' AND trackUrl IS NULL))
);
ALTER TABLE content_publication ADD COLUMN networkMode TEXT NOT NULL DEFAULT 'unconfigured';
ALTER TABLE content_publication ADD COLUMN trackUrl TEXT;
CREATE TRIGGER publication_fixed_network BEFORE UPDATE OF networkMode, trackUrl ON content_publication
BEGIN SELECT RAISE(ABORT, 'publication network is immutable'); END;
