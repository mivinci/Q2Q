CREATE TABLE content_publication (
  id TEXT PRIMARY KEY NOT NULL,
  projectId TEXT NOT NULL REFERENCES project(id),
  fileId TEXT NOT NULL REFERENCES file_record(id),
  uploadId TEXT NOT NULL REFERENCES upload_task(id),
  createdBy TEXT NOT NULL REFERENCES user(id),
  status TEXT NOT NULL CHECK(status IN ('creating', 'published', 'failed', 'withdrawn')),
  name TEXT NOT NULL,
  size INTEGER NOT NULL CHECK(size > 0),
  infoHash TEXT NOT NULL,
  objectKey TEXT NOT NULL,
  objectVersion TEXT,
  objectEtag TEXT NOT NULL,
  transport TEXT NOT NULL,
  bucketIdentity TEXT NOT NULL,
  preparedSeedKey TEXT NOT NULL,
  preparedProofKey TEXT NOT NULL,
  seedDigest TEXT NOT NULL,
  seedSize INTEGER NOT NULL CHECK(seedSize > 0),
  seedKey TEXT,
  proofKey TEXT,
  cleanupPending INTEGER NOT NULL DEFAULT 0 CHECK(cleanupPending IN (0, 1)),
  lastSignedUntil INTEGER NOT NULL DEFAULT 0 CHECK(lastSignedUntil >= 0),
  createdAt INTEGER NOT NULL,
  updatedAt INTEGER NOT NULL,
  withdrawnAt INTEGER,
  CHECK(status <> 'published' OR (seedKey IS NOT NULL AND proofKey IS NOT NULL
    AND seedKey <> preparedSeedKey AND proofKey <> preparedProofKey)),
  CHECK((status = 'withdrawn') = (withdrawnAt IS NOT NULL))
);
CREATE UNIQUE INDEX publication_active_file_idx ON content_publication(fileId)
  WHERE status IN ('creating', 'published');
CREATE INDEX publication_file_created_idx ON content_publication(fileId, createdAt DESC);

-- A version's content identity and source cannot be rewritten by a retry.
CREATE TRIGGER publication_immutable_snapshot BEFORE UPDATE OF
  id, projectId, fileId, uploadId, createdBy, name, size, infoHash, objectKey,
  objectVersion, objectEtag, transport, bucketIdentity, preparedSeedKey,
  preparedProofKey, seedDigest, seedSize, createdAt ON content_publication
BEGIN SELECT RAISE(ABORT, 'publication snapshot is immutable'); END;

CREATE TRIGGER publication_state_transition BEFORE UPDATE OF status ON content_publication
WHEN NEW.status <> OLD.status AND NOT (
  (OLD.status = 'creating' AND NEW.status IN ('published', 'failed', 'withdrawn')) OR
  (OLD.status = 'failed' AND NEW.status IN ('creating', 'withdrawn')) OR
  (OLD.status = 'published' AND NEW.status = 'withdrawn')
)
BEGIN SELECT RAISE(ABORT, 'invalid publication transition'); END;
