CREATE TABLE project_subscription (
  projectId TEXT PRIMARY KEY NOT NULL REFERENCES project(id) ON DELETE CASCADE,
  planId TEXT NOT NULL CHECK(planId IN ('free', 'creator', 'team')),
  status TEXT NOT NULL CHECK(status IN ('simulated', 'active', 'past_due', 'cancelled', 'expired')),
  provider TEXT,
  providerSubscriptionId TEXT,
  periodStartAt INTEGER,
  periodEndAt INTEGER,
  cancelAtPeriodEnd INTEGER NOT NULL DEFAULT 0 CHECK(cancelAtPeriodEnd IN (0, 1)),
  createdAt INTEGER NOT NULL,
  updatedAt INTEGER NOT NULL
);
