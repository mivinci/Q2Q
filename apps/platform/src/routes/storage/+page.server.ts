import { env } from 'cloudflare:workers';
import { fail, redirect } from '@sveltejs/kit';
import type { Actions, PageServerLoad } from './$types';
import { uploadTransport } from '#lib/server/upload-storage.js';
import { requireProject } from '#lib/server/projects.js';
import { enablePlatformStorage, getPlatformStorage } from '#lib/server/storage.js';

export const load: PageServerLoad = async ({ locals, parent }) => {
  if (!locals.user) redirect(303, '/login');
  const { currentProject } = await parent();
  if (!currentProject) redirect(303, '/library');
  return {
    storageConnection: await getPlatformStorage(env.DB, currentProject.id),
    storageAvailable: uploadTransport() !== null
  };
};

export const actions: Actions = {
  enable: async ({ locals, request }) => {
    if (!locals.user) redirect(303, '/login');
    const projectId = (await request.formData()).get('projectId');
    if (typeof projectId !== 'string') return fail(400, { error: 'invalidProject' as const });
    await requireProject(env.DB, locals.user.id, projectId, 'owner');
    if (!uploadTransport()) return fail(503, { error: 'storageUnavailable' as const });
    await enablePlatformStorage(env.DB, projectId);
    redirect(303, '/storage');
  }
};
