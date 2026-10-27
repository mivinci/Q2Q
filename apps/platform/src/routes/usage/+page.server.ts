import { env } from 'cloudflare:workers';
import { fail, redirect } from '@sveltejs/kit';
import type { Actions, PageServerLoad } from './$types';
import { requireProject } from '#lib/server/projects.js';
import { getProjectEntitlements, isPlanId, getPlanCatalog, planIds, type PlanFeature, type PlanId } from '#lib/server/plans.js';
import { getStorageUsage } from '#lib/server/storage-quota.js';
import { billingEnabled, createCheckout, createBillingPortal, redirectToBilling, paidPlan, type PaidPlan } from '#lib/server/creem.js';

export const load: PageServerLoad = async ({ locals, parent, url }) => {
  if (!locals.user) redirect(303, '/login');
  const { currentProject } = await parent();
  if (!currentProject) redirect(303, '/library');

  const [entitlements, usage, storageUsage, planCatalog, billing, pending, customer] = await Promise.all([
    getProjectEntitlements(env.DB, currentProject.id),
    env.DB.prepare(`SELECT COUNT(*) AS files, COALESCE(SUM(size), 0) AS recordedBytes
      FROM file_record WHERE projectId = ? AND status = 'uploaded'`)
      .bind(currentProject.id).first<{ files: number; recordedBytes: number }>(),
    getStorageUsage(env.DB, currentProject.id),
    getPlanCatalog(env.DB),
    env.DB.prepare(`SELECT status, planId FROM billing_checkout
      WHERE projectId=? AND mode='test' AND status IN ('creating','open','unknown','paid')
      ORDER BY createdAt DESC, id DESC LIMIT 1`)
      .bind(currentProject.id).first<{status:string;planId:PaidPlan}>(),
    env.DB.prepare(`SELECT status, planId FROM billing_checkout
      WHERE projectId=? AND mode='test' AND status IN ('creating','open','unknown')
      ORDER BY createdAt DESC, id DESC LIMIT 1`)
      .bind(currentProject.id).first<{status:'creating'|'open'|'unknown';planId:PaidPlan}>(),
    env.DB.prepare(`SELECT 1 AS present FROM billing_subscription
      WHERE projectId=? AND mode='test' LIMIT 1`)
      .bind(currentProject.id).first<{present:number}>()
  ]);

  return {
    currentProject,
    entitlements,
    usage: usage ?? { files: 0, recordedBytes: 0 },
    storageUsage,
    simulationEnabled: env.BILLING_SIMULATION === 'enabled',
    creemEnabled: billingEnabled(env),
    billingReturn: url.searchParams.get('billing') === 'return',
    billing: billing ? {status:billing.status, planId:billing.planId} : null,
    pending,
    hasCustomer: !!customer,
    plans: planIds.map((id) => ({
      id: id as PlanId,
      nameKey: planCatalog[id].nameKey,
      descriptionKey: planCatalog[id].descriptionKey,
      features: [...planCatalog[id].features] as PlanFeature[],
      maxFileSizeBytes: planCatalog[id].maxFileSizeBytes,
      storageBytes: planCatalog[id].storageBytes
    }))
  };
};

export const actions: Actions = {
  portal: async ({ locals, request, url }) => {
    if (!locals.user) redirect(303, '/login');
    if (request.headers.get('origin') !== url.origin) return fail(403, {error:'billingUnavailable' as const});
    const projectId = (await request.formData()).get('projectId');
    if (typeof projectId !== 'string') return fail(400, {error:'billingUnavailable' as const});
    await requireProject(env.DB,locals.user.id,projectId,'owner');
    let destination:string;
    try {destination = await createBillingPortal(env,projectId,locals.user.id);}
    catch {return fail(409,{error:'billingUnavailable' as const});}
    redirectToBilling(destination);
  },
  checkout: async ({ locals, request, url }) => {
    if (!locals.user) redirect(303, '/login');
    if (request.headers.get('origin') !== url.origin) return fail(403, {error: 'billingUnavailable' as const});
    const form = await request.formData();
    const projectId = form.get('projectId'), planId = form.get('planId');
    if (typeof projectId !== 'string' || !paidPlan(planId)) return fail(400, {error: 'billingUnavailable' as const});
    await requireProject(env.DB, locals.user.id, projectId, 'owner');
    let destination: string;
    try { destination = await createCheckout(env, projectId, locals.user.id, planId, url.origin); }
    catch { return fail(409, {error: 'billingUnavailable' as const}); }
    redirectToBilling(destination);
  },
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
        AND NOT EXISTS (SELECT 1 FROM billing_checkout WHERE projectId = ? AND status IN ('creating','open','unknown'))
      ON CONFLICT(projectId) DO UPDATE SET planId = excluded.planId, status = 'simulated', provider = 'local',
        providerSubscriptionId = NULL, periodStartAt = NULL, periodEndAt = NULL, cancelAtPeriodEnd = 0,
        updatedAt = excluded.updatedAt
      WHERE COALESCE(project_subscription.provider, '') != 'creem'
      RETURNING projectId`)
      .bind(projectId, planId, now, now, projectId, locals.user.id, projectId)
      .first<{ projectId: string }>();
    if (!saved) return fail(403, { error: 'billingUnavailable' as const });
    return { simulated: true };
  }
};
