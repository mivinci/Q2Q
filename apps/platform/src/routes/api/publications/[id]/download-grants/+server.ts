import type { RequestHandler } from './$types';
import { uploadUser } from '#lib/server/upload-api.js';
import { createDownloadGrant, listDownloadGrants } from '#lib/server/download-grants.js';
export const GET: RequestHandler = async (event) => Response.json(await listDownloadGrants(event.params.id, uploadUser(event)));
export const POST: RequestHandler = async (event) => Response.json(await createDownloadGrant(event.params.id, uploadUser(event), event.url.origin), { status: 201, headers: { 'Referrer-Policy': 'no-referrer' } });
