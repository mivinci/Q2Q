-- Pre-release Creem test billing. A checkout attempt and a Creem subscription
-- have separate lifecycles: a project may buy again after a subscription ends,
-- and a late event for an old subscription must not change the current
-- entitlement. Every row is test mode until production billing is agreed.
CREATE TABLE billing_checkout (
  id TEXT PRIMARY KEY NOT NULL,
  projectId TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
  ownerId TEXT NOT NULL REFERENCES user(id),
  mode TEXT NOT NULL CHECK(mode IN ('test', 'live')),
  planId TEXT NOT NULL CHECK(planId IN ('creator', 'team')),
  productId TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('creating', 'open', 'unknown', 'paid', 'closed')),
  checkoutId TEXT UNIQUE,
  checkoutUrl TEXT,
  reconcileToken TEXT,
  reconcileUntil INTEGER,
  createdAt INTEGER NOT NULL,
  updatedAt INTEGER NOT NULL
);
-- Only in-flight attempts contend for the project. Paid and closed attempts
-- stay as history, so the project can start another checkout later.
CREATE UNIQUE INDEX billing_one_pending_checkout ON billing_checkout(projectId)
  WHERE status IN ('creating', 'open', 'unknown');

CREATE TABLE billing_subscription (
  subscriptionId TEXT PRIMARY KEY NOT NULL,
  checkoutId TEXT NOT NULL REFERENCES billing_checkout(id),
  projectId TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
  mode TEXT NOT NULL CHECK(mode IN ('test', 'live')),
  planId TEXT NOT NULL CHECK(planId IN ('creator', 'team')),
  productId TEXT NOT NULL,
  customerId TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('active', 'cancelled', 'expired')),
  providerStatus TEXT NOT NULL,
  periodStartAt INTEGER NOT NULL,
  periodEndAt INTEGER NOT NULL,
  cancelAtPeriodEnd INTEGER NOT NULL CHECK(cancelAtPeriodEnd IN (0, 1)),
  reconcileToken TEXT,
  reconcileUntil INTEGER,
  createdAt INTEGER NOT NULL,
  updatedAt INTEGER NOT NULL
);
CREATE INDEX billing_subscription_project ON billing_subscription(projectId, createdAt);

CREATE TABLE billing_webhook_event (
  id TEXT PRIMARY KEY NOT NULL,
  eventType TEXT NOT NULL,
  bodyHash TEXT NOT NULL,
  checkoutId TEXT REFERENCES billing_checkout(id) ON DELETE SET NULL,
  subscriptionId TEXT,
  receivedAt INTEGER NOT NULL,
  processedAt INTEGER
);
CREATE INDEX billing_webhook_event_subscription ON billing_webhook_event(subscriptionId);

-- Protect in-flight purchases and provider subscriptions from the existing
-- manual and simulation writers. Webhook writes identify themselves as creem.
CREATE TRIGGER billing_subscription_guard_update
BEFORE UPDATE ON project_subscription
WHEN (OLD.provider = 'creem' OR EXISTS (
  SELECT 1 FROM billing_checkout WHERE projectId = OLD.projectId AND status IN ('creating','open','unknown')
)) AND COALESCE(NEW.provider, '') != 'creem'
BEGIN SELECT RAISE(ABORT, 'billing subscription conflict'); END;

CREATE TRIGGER billing_subscription_guard_insert
BEFORE INSERT ON project_subscription
WHEN EXISTS (SELECT 1 FROM billing_checkout WHERE projectId = NEW.projectId AND status IN ('creating','open','unknown'))
  AND COALESCE(NEW.provider, '') != 'creem'
BEGIN SELECT RAISE(ABORT, 'billing subscription conflict'); END;
