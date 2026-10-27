import { env } from 'cloudflare:workers';
import type { PageServerLoad } from './$types';
import { overview } from '#lib/server/queries.js';
export const load: PageServerLoad = async () => ({ summary: await overview(env.PLATFORM_DB), observedAt:Date.now() });
