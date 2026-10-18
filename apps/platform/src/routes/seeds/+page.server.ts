import { redirect } from '@sveltejs/kit';
import type { PageServerLoad } from './$types';
import { listProjectPublications } from '#lib/server/publications.js';
export const load: PageServerLoad = async ({ locals, parent }) => {
  if (!locals.user) redirect(303, '/login');
  const { currentProject } = await parent();
  return { seeds: currentProject ? await listProjectPublications(currentProject.id, locals.user.id) : [] };
};
