CREATE TABLE download_grant (
  id TEXT PRIMARY KEY NOT NULL,
  publicationId TEXT NOT NULL REFERENCES content_publication(id),
  secretDigest TEXT NOT NULL,
  createdBy TEXT NOT NULL REFERENCES user(id),
  createdAt INTEGER NOT NULL,
  revokedAt INTEGER
);
CREATE INDEX download_grant_publication_idx ON download_grant(publicationId, createdAt DESC);
CREATE TRIGGER download_grant_immutable BEFORE UPDATE OF id, publicationId, secretDigest, createdBy, createdAt ON download_grant
BEGIN SELECT RAISE(ABORT, 'download grant is immutable'); END;
CREATE TRIGGER download_grant_terminal BEFORE UPDATE OF revokedAt ON download_grant
WHEN OLD.revokedAt IS NOT NULL AND NEW.revokedAt IS NOT OLD.revokedAt
BEGIN SELECT RAISE(ABORT, 'grant revocation is terminal'); END;
