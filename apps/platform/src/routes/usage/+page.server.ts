import { env } from 'cloudflare:workers';
import { fail, redirect } from '@sveltejs/kit';
import type { Actions, PageServerLoad } from './$types';
import { requireProject } from '#lib/server/projects.js';
import { getProjectEntitlements, isPlanId, planCatalog, planIds, type PlanFeature, type PlanId } from '#lib/server/plans.js';

export const load: PageServerLoad = async ({ locals, parent }) => {
  if (!locals.user) redirect(303, '/login');
  const { currentProject } = await parent();
  if (!currentProject) redirect(303, '/library');

  const [entitlements, usage] = await Promise.all([
    getProjectEntitlements(env.DB, currentProject.id),
    env.DB.prepare(`SELECT COUNT(*) AS files, COALESCE(SUM(size), 0) AS recordedBytes
      FROM file_record WHERE projectId = ? AND status = 'uploaded'`)
      .bind(currentProject.id).first<{ files: number; recordedBytes: number }>()
  ]);

  return {
    currentProject,
    entitlements,
    usage: usage ?? { files: 0, recordedBytes: 0 },
    simulationEnabled: env.BILLING_SIMULATION === 'enabled',
    plans: planIds.map((id) => ({
      id: id as PlanId,
      nameKey: planCatalog[id].nameKey,
      descriptionKey: planCatalog[id].descriptionKey,
      features: [...planCatalog[id].features] as PlanFeature[],
      maxFileSizeBytes: planCatalog[id].maxFileSizeBytes
    }))
  };
};

export const actions: Actions = {
  simulate: async ({ locals, request, url }) => {
    if (!locals.user) redirect(303, '/login');
    if (env.BILLING_SIMULATION !== 'enabled') return fail(404, { error: 'billingUnavailable' as const });
    if (request.headers.get('origin') !== url.origin) return fail(403, { error: 'billingUnavailable' as const });

    const form = await request.formData();
    const projectId = form.get('projectId');
    const planId = form.get('planId');
    if (typeof projectId !== 'string' || !isPlanId(planId)) return fail(400, { error: 'billingUnavailable' as const });
    await requireProject(env.DB, locals.user.id, projectId, 'owner');

    const now = Date.now();
    // The membership and owner checks are repeated in the write to close the permission-change race.
    const saved = await env.DB.prepare(`INSERT INTO project_subscription
      (projectId, planId, status, provider, periodStartAt, periodEndAt, cancelAtPeriodEnd, createdAt, updatedAt)
      SELECT ?, ?, 'simulated', 'local', NULL, NULL, 0, ?, ?
      WHERE EXISTS (SELECT 1 FROM project_member WHERE projectId = ? AND userId = ? AND role = 'owner')
      ON CONFLICT(projectId) DO UPDATE SET planId = excluded.planId, status = 'simulated', provider = 'local',
        providerSubscriptionId = NULL, periodStartAt = NULL, periodEndAt = NULL, cancelAtPeriodEnd = 0,
        updatedAt = excluded.updatedAt
      RETURNING projectId`)
      .bind(projectId, planId, now, now, projectId, locals.user.id)
      .first<{ projectId: string }>();
    if (!saved) return fail(403, { error: 'billingUnavailable' as const });
    return { simulated: true };
  }
};
