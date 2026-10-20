-- One latest snapshot per registered Punch or Track. Read-only clients are not nodes.
CREATE TABLE nodes (
  node_id TEXT PRIMARY KEY NOT NULL,
  service_type TEXT NOT NULL CHECK (service_type IN ('punch', 'track')),
  identity_version INTEGER NOT NULL CHECK (identity_version BETWEEN 1 AND 9007199254740991),
  advertise_url TEXT NOT NULL,
  public_key TEXT,
  version TEXT NOT NULL,
  region TEXT,
  accepting_new_connections INTEGER NOT NULL CHECK (accepting_new_connections IN (0, 1)),
  metrics_json TEXT CHECK (metrics_json IS NULL OR (json_valid(metrics_json) AND json_type(metrics_json) = 'object')),
  created_at INTEGER NOT NULL CHECK (created_at >= 0),
  last_seen INTEGER NOT NULL CHECK (last_seen >= created_at),
  expires_at INTEGER NOT NULL CHECK (expires_at > last_seen AND expires_at <= last_seen + 90),
  CHECK (
    (service_type = 'punch' AND public_key IS NOT NULL AND length(public_key) = 44)
    OR (service_type = 'track' AND public_key IS NULL)
  )
);

-- Discovery excludes expired nodes. Public/display lists paginate by the primary key.
CREATE INDEX nodes_discovery ON nodes(service_type, expires_at, node_id);
