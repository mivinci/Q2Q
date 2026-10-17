import type { RequestHandler } from './$types';
import { downloadLocation } from '#lib/server/download-grants.js';
export const GET: RequestHandler = async (event) => new Response(null, { status: 307, headers: {
  Location: await downloadLocation(event.params.id, event.url.searchParams.get('grant') ?? '', event.url.searchParams.get('secret') ?? '', event.request.headers.get('range')),
  'Cache-Control': 'private, no-store', 'Referrer-Policy': 'no-referrer', 'X-Content-Type-Options': 'nosniff'
} });
