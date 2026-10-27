import { env } from 'cloudflare:workers';
import { fail, redirect } from '@sveltejs/kit';
import type { Actions, PageServerLoad } from './$types';
import { projectNetwork, normalizeTrack } from '#lib/server/network.js';
import { requireProject } from '#lib/server/projects.js';
import { requireProjectFeature, effectivePlanIdSql } from '#lib/server/plans.js';
export const load: PageServerLoad = async ({ locals, parent }) => {
  if (!locals.user) redirect(303, '/login');
  const { currentProject } = await parent();
  if (!currentProject) redirect(303, '/library');
  return { network: await projectNetwork(currentProject.id) };
};
export const actions: Actions = {
  save: async ({ locals, request, url }) => {
    if (!locals.user) redirect(303, '/login');
    if (request.headers.get('origin') !== url.origin) return fail(403, { error: 'networkInvalid' as const });
    const form = await request.formData();
    const projectId = form.get('projectId'), mode = form.get('mode');
    if (typeof projectId !== 'string' || (mode !== 'platform' && mode !== 'custom')) return fail(400, { error: 'networkInvalid' as const });
    await requireProject(env.DB, locals.user.id, projectId, 'owner');
    await requireProjectFeature(env.DB, projectId, mode === 'platform' ? 'platformNetwork' : 'customTrack');
    let track: string | null = null;
    try { if (mode === 'custom') track = normalizeTrack(String(form.get('trackUrl') ?? '').trim()); }
    catch { return fail(400, { error: 'networkInvalid' as const }); }
    // Membership or plan access may change after preflight; arbitrate in the write.
    const now = Date.now();
    const saved = await env.DB.prepare(`INSERT INTO project_network (projectId, mode, trackUrl, updatedAt)
      SELECT ?, ?, ?, ? WHERE EXISTS (SELECT 1 FROM project_member WHERE projectId = ? AND userId = ? AND role = 'owner')
      AND EXISTS (SELECT 1 FROM plan_catalog WHERE id = ${effectivePlanIdSql('?', String(now))} AND ${mode === 'custom' ? 'customTrack' : 'platformNetwork'} = 1)
      ON CONFLICT(projectId) DO UPDATE SET mode = excluded.mode, trackUrl = excluded.trackUrl, updatedAt = excluded.updatedAt RETURNING projectId`)
      .bind(projectId, mode, track, now, projectId, locals.user.id, projectId).first();
    if (!saved) return fail(403, { error: 'networkInvalid' as const });
    return { saved: true };
  }
};
