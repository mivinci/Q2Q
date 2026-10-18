import { error, redirect } from '@sveltejs/kit';
import { env } from 'cloudflare:workers';
import type { PageServerLoad } from './$types';
import { requirePublication } from '#lib/server/publications.js';
export const load: PageServerLoad = async ({ locals, params, parent }) => {
  if (!locals.user) redirect(303, '/login');
  const { currentProject } = await parent();
  const seed = await requirePublication(params.id, locals.user.id);
  if (seed.projectId !== currentProject?.id) error(404, 'publicationNotFound');
  // Published history remains readable even after its source file has been tombstoned.
  const file = await env.DB.prepare('SELECT createdBy, status FROM file_record WHERE id = ?').bind(seed.fileId).first<{ createdBy: string; status: string }>();
  return { seed, sourceAvailable: !!file && file.status !== 'deleted', canManage: file?.createdBy === locals.user.id || currentProject.role === 'owner' };
};
