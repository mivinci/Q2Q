# @q2q/plans

Platform and admin share the D1 plan catalog reader, entitlement types and subscription SQL rules here. The package has no UI, router or authentication dependency; callers independently authorize access to project data and translate `PlanConfigurationError` to HTTP 503.

`getPlanCatalog(db)` reads and validates all three definitions on each call. No code-default quota fallback or process cache is used. Stable plan IDs and translation keys remain in code; limits, feature flags, revision and update time live in `plan_catalog`. `uploadMaxSize` is the implemented upload ceiling, not a plan allowance.

`getProjectEntitlements(db, projectId, now)` applies `active`/`simulated` only when the optional start is at or before `now` and the optional end is after it. Null dates are unbounded. Other subscriptions and missing subscriptions use Free. Cancelling at period end does not revoke an otherwise active period early. A missing or invalid catalog is a service error.

`activeSubscriptionSql` and `effectivePlanIdSql` generate matching rules for atomic upload reservations, invitations and admin queries. Their expression arguments are internal SQL fragments, never request values. Bind user input and use one captured timestamp per operation.

Apply platform migration `0017_plan_catalog.sql` before deploying consumers. It seeds existing definitions and preserves project subscription rows. Admin definition editing and manual subscriptions use an atomic audit transaction, with revision and idempotency checks; apply migration `0018_admin_operations.sql` before enabling these controls.
