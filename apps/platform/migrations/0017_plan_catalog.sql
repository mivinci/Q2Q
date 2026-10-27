-- Stable plan IDs remain attached to existing project subscriptions.
-- Definitions are editable in place; revision is for optimistic concurrency.
CREATE TABLE plan_catalog (
  id TEXT PRIMARY KEY NOT NULL CHECK(id IN ('free', 'creator', 'team')),
  maxFileSizeBytes INTEGER NOT NULL CHECK(typeof(maxFileSizeBytes) = 'integer' AND maxFileSizeBytes BETWEEN 1 AND 8589934592),
  storageBytes INTEGER NOT NULL CHECK(typeof(storageBytes) = 'integer' AND storageBytes BETWEEN 1 AND 9007199254740991 AND storageBytes >= maxFileSizeBytes),
  platformStorage INTEGER NOT NULL CHECK(platformStorage IN (0, 1)),
  platformNetwork INTEGER NOT NULL CHECK(platformNetwork IN (0, 1)),
  customTrack INTEGER NOT NULL CHECK(customTrack IN (0, 1)),
  higherLimits INTEGER NOT NULL CHECK(higherLimits IN (0, 1)),
  teamCollaboration INTEGER NOT NULL CHECK(teamCollaboration IN (0, 1)),
  revision INTEGER NOT NULL DEFAULT 1 CHECK(typeof(revision) = 'integer' AND revision BETWEEN 1 AND 9007199254740991),
  updatedAt INTEGER NOT NULL CHECK(typeof(updatedAt) = 'integer' AND updatedAt BETWEEN 0 AND 9007199254740991)
);

INSERT INTO plan_catalog
  (id, maxFileSizeBytes, storageBytes, platformStorage, platformNetwork, customTrack, higherLimits, teamCollaboration, updatedAt)
VALUES
  ('free', 2097152, 104857600, 1, 1, 1, 0, 0, CAST(strftime('%s','now') AS INTEGER) * 1000),
  ('creator', 8589934592, 107374182400, 1, 1, 1, 1, 0, CAST(strftime('%s','now') AS INTEGER) * 1000),
  ('team', 8589934592, 1099511627776, 1, 1, 1, 1, 1, CAST(strftime('%s','now') AS INTEGER) * 1000);
