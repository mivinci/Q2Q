import { env } from 'cloudflare:workers';
import { error } from '@sveltejs/kit';
import { effectivePlanIdSql, isPlanId } from '@q2q/plans';
import { plans } from '#lib/server/queries.js';
import { updatePlan, planFields } from '#lib/server/mutations.js';
import { mutationAction } from '#lib/server/mutation-action.js';
import type { PageServerLoad, Actions } from './$types';

export const load: PageServerLoad = async ({ params }) => {
  if (!isPlanId(params.id)) error(404, 'Record not found');
  const definitions = await plans(env.PLATFORM_DB);
  const plan = definitions.find(item => item.id === params.id)!;
  const now = Date.now();
  const usage = await env.PLATFORM_DB.prepare(`SELECT COUNT(*) AS count FROM project p WHERE ${effectivePlanIdSql('p.id', '?')} = ?`)
    .bind(now, now, params.id).first<{ count: number }>();
  return { plan, affectedProjects: usage?.count ?? 0, requestId: crypto.randomUUID() };
};
export const actions: Actions = {
  default: ({ request, url, locals, params }) => mutationAction(request, url, locals.admin, planFields,
    form => updatePlan(env.PLATFORM_DB, locals.admin, params.id, form))
};
