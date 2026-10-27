// This is the implemented upload protocol ceiling, not a subscription allowance.
export const uploadMaxSize = 8 * 1024 ** 3;
export const planIds = ['free', 'creator', 'team'] as const;
export type PlanId = (typeof planIds)[number];
export const planFeatures = ['platformStorage', 'platformNetwork', 'customTrack', 'higherLimits', 'teamCollaboration'] as const;
export type PlanFeature = (typeof planFeatures)[number];
export type SubscriptionStatus = 'simulated' | 'active' | 'past_due' | 'cancelled' | 'expired';
export const planMetadata = {
  free: { nameKey: 'planFree', descriptionKey: 'planFreeDescription' },
  creator: { nameKey: 'planCreator', descriptionKey: 'planCreatorDescription' },
  team: { nameKey: 'planTeam', descriptionKey: 'planTeamDescription' }
} as const;

export interface PlanDefinition {
  nameKey: typeof planMetadata[PlanId]['nameKey'];
  descriptionKey: typeof planMetadata[PlanId]['descriptionKey'];
  features: PlanFeature[];
  maxFileSizeBytes: number;
  storageBytes: number;
  revision: number;
  updatedAt: number;
}
export type PlanCatalog = Record<PlanId, PlanDefinition>;

export class PlanConfigurationError extends Error {
  readonly status = 503;
  constructor() { super('Plan catalog unavailable'); }
}

export function isPlanId(value: unknown): value is PlanId {
  return typeof value === 'string' && planIds.includes(value as PlanId);
}

export async function getPlanCatalog(db: D1Database): Promise<PlanCatalog> {
  try {
    const { results } = await db.prepare(`SELECT id, maxFileSizeBytes, storageBytes,
      platformStorage, platformNetwork, customTrack, higherLimits, teamCollaboration,
      revision, updatedAt FROM plan_catalog ORDER BY id`).all<Record<string, unknown>>();
    if (results.length !== planIds.length) throw new PlanConfigurationError();
    const catalog = {} as PlanCatalog;
    for (const row of results) {
      if (!isPlanId(row.id) || catalog[row.id] ||
          !Number.isSafeInteger(row.maxFileSizeBytes) || Number(row.maxFileSizeBytes) < 1 || Number(row.maxFileSizeBytes) > uploadMaxSize ||
          !Number.isSafeInteger(row.storageBytes) || Number(row.storageBytes) < Number(row.maxFileSizeBytes) ||
          !Number.isSafeInteger(row.revision) || Number(row.revision) < 1 ||
          !Number.isSafeInteger(row.updatedAt) || Number(row.updatedAt) < 0 ||
          planFeatures.some(feature => row[feature] !== 0 && row[feature] !== 1)) throw new PlanConfigurationError();
      catalog[row.id] = {
        ...planMetadata[row.id],
        features: planFeatures.filter(feature => row[feature] === 1),
        maxFileSizeBytes: Number(row.maxFileSizeBytes), storageBytes: Number(row.storageBytes),
        revision: Number(row.revision), updatedAt: Number(row.updatedAt)
      };
    }
    return catalog;
  } catch { throw new PlanConfigurationError(); }
}

// Expressions below are internal SQL fragments, never request parameters. Passing
// one captured timestamp keeps entitlement reads and atomic writes on one rule.
export function activeSubscriptionSql(alias: string, now: string): string {
  return `${alias}.status IN ('active', 'simulated')
    AND (${alias}.periodStartAt IS NULL OR ${alias}.periodStartAt <= ${now})
    AND (${alias}.periodEndAt IS NULL OR ${alias}.periodEndAt > ${now})`;
}

export function effectivePlanIdSql(project: string, now: string): string {
  return `COALESCE((SELECT subscription.planId FROM project_subscription subscription
    WHERE subscription.projectId = ${project} AND ${activeSubscriptionSql('subscription', now)}), 'free')`;
}

interface SubscriptionRow {
  planId: PlanId;
  status: SubscriptionStatus;
  periodStartAt: number | null;
  periodEndAt: number | null;
  cancelAtPeriodEnd: number;
  effective: number;
}
export interface ProjectEntitlements {
  planId: PlanId;
  status: SubscriptionStatus | 'free';
  periodStartAt: number | null;
  periodEndAt: number | null;
  cancelAtPeriodEnd: boolean;
  features: Record<PlanFeature, boolean>;
  limits: { maxFileSizeBytes: number; storageBytes: number; retentionDays: null; monthlyEgressBytes: null; monthlyTurnBytes: null; concurrentTransfers: null };
}

export async function getProjectEntitlements(db: D1Database, projectId: string, now = Date.now()): Promise<ProjectEntitlements> {
  const [catalog, row] = await Promise.all([
    getPlanCatalog(db),
    db.prepare(`SELECT planId, status, periodStartAt, periodEndAt, cancelAtPeriodEnd,
      CASE WHEN ${activeSubscriptionSql('s', '?')} THEN 1 ELSE 0 END AS effective
      FROM project_subscription s WHERE projectId = ?`).bind(now, now, projectId).first<SubscriptionRow>()
  ]);
  const activeRow = row?.effective ? row : null;
  const planId = activeRow?.planId ?? 'free';
  const plan = catalog[planId];
  if (!plan) throw new PlanConfigurationError();
  return {
    planId, status: activeRow?.status ?? 'free',
    periodStartAt: activeRow?.periodStartAt ?? null, periodEndAt: activeRow?.periodEndAt ?? null,
    cancelAtPeriodEnd: activeRow?.cancelAtPeriodEnd === 1,
    features: {
      platformStorage: plan.features.includes('platformStorage'),
      platformNetwork: plan.features.includes('platformNetwork'),
      customTrack: plan.features.includes('customTrack'),
      higherLimits: plan.features.includes('higherLimits'),
      teamCollaboration: plan.features.includes('teamCollaboration')
    },
    // Null means no product limit has been defined, not unlimited usage.
    limits: { maxFileSizeBytes: plan.maxFileSizeBytes, storageBytes: plan.storageBytes,
      retentionDays: null, monthlyEgressBytes: null, monthlyTurnBytes: null, concurrentTransfers: null }
  };
}
