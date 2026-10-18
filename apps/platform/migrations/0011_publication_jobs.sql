ALTER TABLE content_publication ADD COLUMN leaseToken TEXT;
ALTER TABLE content_publication ADD COLUMN leaseUntil INTEGER NOT NULL DEFAULT 0;
ALTER TABLE content_publication ADD COLUMN errorCode TEXT;

-- Keep every attempt durable, including a worker that dies before committing its result.
CREATE TABLE publication_artifact (
  token TEXT PRIMARY KEY NOT NULL,
  publicationId TEXT NOT NULL REFERENCES content_publication(id),
  seedKey TEXT NOT NULL,
  proofKey TEXT NOT NULL,
  createdAt INTEGER NOT NULL
);
CREATE INDEX publication_artifact_version_idx ON publication_artifact(publicationId);

CREATE TRIGGER publication_fixed_artifacts BEFORE UPDATE OF seedKey, proofKey ON content_publication
WHEN OLD.status IN ('published', 'withdrawn') AND
  (NEW.seedKey IS NOT OLD.seedKey OR NEW.proofKey IS NOT OLD.proofKey)
BEGIN SELECT RAISE(ABORT, 'published artifacts are immutable'); END;
