import { redirect } from '@sveltejs/kit';
import type { PageServerLoad } from './$types';
import { getRegistryNodes } from '#lib/server/registry-nodes.js';

export const load: PageServerLoad = async ({ locals }) => {
  if (!locals.user) redirect(303, '/login');
  return getRegistryNodes();
};
