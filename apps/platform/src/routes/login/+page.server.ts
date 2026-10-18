import { env } from 'cloudflare:workers';
import { redirect } from '@sveltejs/kit';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = ({ locals }) => {
  if (locals.user) redirect(303, '/');
  return { googleAuthReady: !!(env?.GOOGLE_CLIENT_ID && env?.GOOGLE_CLIENT_SECRET) };
};
