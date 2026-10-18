import { error } from '@sveltejs/kit';
import { uploadMaxSize } from '../upload.js';

export const planIds = ['free', 'creator', 'team'] as const;
export type PlanId = (typeof planIds)[number];
export type PlanFeature = 'platformStorage' | 'platformNetwork' | 'customTrack' | 'higherLimits' | 'teamCollaboration';
export type SubscriptionStatus = 'simulated' | 'active' | 'past_due' | 'cancelled' | 'expired';
type PlanNameKey = 'planFree' | 'planCreator' | 'planTeam';
type PlanDescriptionKey = 'planFreeDescription' | 'planCreatorDescription' | 'planTeamDescription';

export const planCatalog: Record<PlanId, { nameKey: PlanNameKey; descriptionKey: PlanDescriptionKey; features: PlanFeature[]; maxFileSizeBytes: number; storageBytes: number }> = {
  free: {
    nameKey: 'planFree',
    descriptionKey: 'planFreeDescription',
    features: ['platformStorage', 'platformNetwork', 'customTrack'],
    // Leave CPU headroom for Free-tier seed generation under the 10 ms Worker budget.
    maxFileSizeBytes: 2 * 1024 * 1024,
    storageBytes: 100 * 1024 * 1024
  },
  creator: {
    nameKey: 'planCreator',
    descriptionKey: 'planCreatorDescription',
    features: ['platformStorage', 'platformNetwork', 'customTrack', 'higherLimits'],
    maxFileSizeBytes: uploadMaxSize,
    storageBytes: 100 * 1024 ** 3
  },
  team: {
    nameKey: 'planTeam',
    descriptionKey: 'planTeamDescription',
    features: ['platformStorage', 'platformNetwork', 'customTrack', 'higherLimits', 'teamCollaboration'],
    maxFileSizeBytes: uploadMaxSize,
    storageBytes: 1024 ** 4
  }
};

interface SubscriptionRow {
  planId: PlanId;
  status: SubscriptionStatus;
  periodStartAt: number | null;
  periodEndAt: number | null;
  cancelAtPeriodEnd: number;
}

export interface ProjectEntitlements {
  planId: PlanId;
  status: SubscriptionStatus | 'free';
  periodStartAt: number | null;
  periodEndAt: number | null;
  cancelAtPeriodEnd: boolean;
  features: Record<PlanFeature, boolean>;
  // Null means the product limit has not been decided; it does not mean unlimited.
  limits: { maxFileSizeBytes: number; storageBytes: number; retentionDays: null; monthlyEgressBytes: null; monthlyTurnBytes: null; concurrentTransfers: null };
}

export async function getProjectEntitlements(db: D1Database, projectId: string): Promise<ProjectEntitlements> {
  const row = await db.prepare(`SELECT planId, status, periodStartAt, periodEndAt, cancelAtPeriodEnd
    FROM project_subscription WHERE projectId = ?`).bind(projectId).first<SubscriptionRow>();

  // A lapsed or failed subscription falls back to Free. Recovery and export paths remain available.
  const activeRow = row && (row.status === 'active' || row.status === 'simulated') ? row : null;
  const planId = activeRow?.planId ?? 'free';
  const included = new Set(planCatalog[planId].features);
  return {
    planId,
    status: activeRow?.status ?? 'free',
    periodStartAt: activeRow?.periodStartAt ?? null,
    periodEndAt: activeRow?.periodEndAt ?? null,
    cancelAtPeriodEnd: activeRow?.cancelAtPeriodEnd === 1,
    features: {
      platformStorage: included.has('platformStorage'),
      platformNetwork: included.has('platformNetwork'),
      customTrack: included.has('customTrack'),
      higherLimits: included.has('higherLimits'),
      teamCollaboration: included.has('teamCollaboration')
    },
    limits: { maxFileSizeBytes: planCatalog[planId].maxFileSizeBytes, storageBytes: planCatalog[planId].storageBytes, retentionDays: null, monthlyEgressBytes: null, monthlyTurnBytes: null, concurrentTransfers: null }
  };
}

export async function requireProjectFeature(db: D1Database, projectId: string, feature: PlanFeature): Promise<void> {
  const entitlements = await getProjectEntitlements(db, projectId);
  if (!entitlements.features[feature]) error(403, 'planRequired');
}

export function isPlanId(value: unknown): value is PlanId {
  return typeof value === 'string' && planIds.includes(value as PlanId);
}
