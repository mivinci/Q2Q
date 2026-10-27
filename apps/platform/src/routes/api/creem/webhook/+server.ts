import { env } from 'cloudflare:workers';
import { handleCreemWebhook } from '#lib/server/creem.js';
import type { RequestHandler } from './$types';

export const POST: RequestHandler = ({request}) => handleCreemWebhook(env, request);
