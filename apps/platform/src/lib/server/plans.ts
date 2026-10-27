import { error } from '@sveltejs/kit';
import { getProjectEntitlements as readEntitlements, getPlanCatalog as readCatalog, PlanConfigurationError, type PlanFeature } from '@q2q/plans';
export * from '@q2q/plans';

export async function getPlanCatalog(db: D1Database) {
  try { return await readCatalog(db); }
  catch (cause) { if (cause instanceof PlanConfigurationError) error(503, 'Plan catalog unavailable'); throw cause; }
}
export async function getProjectEntitlements(db: D1Database, projectId: string, now = Date.now()) {
  try { return await readEntitlements(db, projectId, now); }
  catch (cause) { if (cause instanceof PlanConfigurationError) error(503, 'Plan catalog unavailable'); throw cause; }
}
export async function requireProjectFeature(db: D1Database, projectId: string, feature: PlanFeature): Promise<void> {
  const entitlements = await getProjectEntitlements(db, projectId);
  if (!entitlements.features[feature]) error(403, 'planRequired');
}
