-- Shared GCRA state: one permit every six seconds, with a maximum burst of three.
-- Keep separate from node leases: rejected reports must never renew online state.
CREATE TABLE report_limits (
  node_id TEXT PRIMARY KEY NOT NULL,
  theoretical_arrival INTEGER NOT NULL
    CHECK (theoretical_arrival BETWEEN 0 AND 9007199254740991)
);
