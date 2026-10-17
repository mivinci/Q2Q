ALTER TABLE publication_artifact ADD COLUMN retiredAt INTEGER;
ALTER TABLE publication_artifact ADD COLUMN checkedAt INTEGER NOT NULL DEFAULT 0;
CREATE INDEX publication_artifact_scan_idx ON publication_artifact(checkedAt, token);
