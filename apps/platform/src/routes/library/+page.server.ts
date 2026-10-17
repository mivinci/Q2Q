import { env } from 'cloudflare:workers';
import { fail, redirect } from '@sveltejs/kit';
import type { Actions, PageServerLoad } from './$types';
import { listFiles } from '#lib/server/files.js';
import { uploadTransport } from '#lib/server/upload-storage.js';
import { getPlatformStorage } from '#lib/server/storage.js';
import { createProject, requireProject } from '#lib/server/projects.js';

const projectCookie = 'q2q-platform-project';
const returnPath = (value: FormDataEntryValue | null) =>
  value === '/storage' || value === '/settings' || value === '/seeds' || value === '/network' ? value : '/library';

export const load: PageServerLoad = async ({ locals, parent }) => {
  if (!locals.user) redirect(303, '/login');
  const { currentProject } = await parent();
  if (!currentProject) redirect(303, '/login');
  const connection = await getPlatformStorage(env.DB, currentProject.id);
  const transport = uploadTransport();
  return { files: await listFiles(currentProject.id, locals.user.id), uploadReady: !!connection && !!transport,
    storageEnabled: !!connection, localUploads: transport === 'local' };
};

export const actions: Actions = {
  createProject: async ({ locals, request, cookies, url }) => {
    if (!locals.user) redirect(303, '/login');
    const form = await request.formData();
    const value = form.get('name');
    if (typeof value !== 'string' || !value.trim() || value.trim().length > 80) {
      return fail(400, { error: 'invalidProjectName' as const });
    }
    const id = await createProject(env.DB, locals.user.id, value.trim());
    cookies.set(projectCookie, id, { path: '/', httpOnly: true, sameSite: 'lax', secure: url.protocol === 'https:', maxAge: 31536000 });
    redirect(303, returnPath(form.get('returnTo')));
  },
  switchProject: async ({ locals, request, cookies, url }) => {
    if (!locals.user) redirect(303, '/login');
    const form = await request.formData();
    const id = form.get('projectId');
    if (typeof id !== 'string') return fail(400, { error: 'invalidProject' as const });
    await requireProject(env.DB, locals.user.id, id);
    cookies.set(projectCookie, id, { path: '/', httpOnly: true, sameSite: 'lax', secure: url.protocol === 'https:', maxAge: 31536000 });
    redirect(303, returnPath(form.get('returnTo')));
  }
};
