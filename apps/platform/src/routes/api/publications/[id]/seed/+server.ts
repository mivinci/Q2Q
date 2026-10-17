import { env } from 'cloudflare:workers';
import { error } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import { requirePublication } from '#lib/server/publications.js';
import { uploadStorage } from '#lib/server/upload-storage.js';
import { uploadUser } from '#lib/server/upload-api.js';
export const GET: RequestHandler = async (event) => {
  await requirePublication(event.params.id, uploadUser(event), true);
  const row = await env.DB.prepare(`SELECT seedKey, seedSize, seedDigest, transport, bucketIdentity
    FROM content_publication WHERE id = ? AND status = 'published'`).bind(event.params.id)
    .first<{ seedKey: string; seedSize: number; seedDigest: string; transport: string; bucketIdentity: string }>();
  if (!row || row.seedSize > 2 * 1024 * 1024) error(409, 'publicationNotReady');
  const object = await uploadStorage(row).get(row.seedKey);
  if (!object.ok || !object.body) error(502, 'processingFailed');
  const reader = object.body.getReader();
  const bytes = new Uint8Array(row.seedSize);
  let offset = 0;
  try {
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      if (offset + value.length > bytes.length) error(502, 'processingFailed');
      bytes.set(value, offset); offset += value.length;
    }
  } finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
  const digest = Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', bytes)), (byte) => byte.toString(16).padStart(2, '0')).join('');
  if (offset !== bytes.length || digest !== row.seedDigest) error(502, 'processingFailed');
  return new Response(bytes, { headers: { 'Content-Type': 'application/octet-stream', 'Cache-Control': 'private, no-store', 'X-Content-Type-Options': 'nosniff' } });
};
