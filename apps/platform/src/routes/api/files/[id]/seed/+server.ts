import { env } from 'cloudflare:workers';
import { error } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import { requireFile } from '#lib/server/files.js';
import { uploadUser } from '#lib/server/upload-api.js';
import { uploadStorage } from '#lib/server/upload-storage.js';

export const GET: RequestHandler = async (event) => {
  const file = await requireFile(event.params.id, uploadUser(event));
  if (file.status !== 'uploaded') error(404, 'fileNotFound');
  const task = await env.DB.prepare(`SELECT s.seedKey, s.seedDigest, s.seedSize, u.transport, u.bucketIdentity FROM seed_task s
    JOIN upload_task u ON u.id = s.uploadId WHERE s.fileId = ? AND s.status = 'ready'`).bind(file.id)
    .first<{ seedKey: string; seedDigest: string | null; seedSize: number | null; transport: string; bucketIdentity: string }>();
  if (!task || !task.seedDigest || !task.seedSize || task.seedSize > 2 * 1024 * 1024) error(409, 'processingNotReady');
  const object = await uploadStorage(task).get(task.seedKey);
  if (!object.body) error(502, 'processingFailed');
  const reader = object.body.getReader();
  const seed = new Uint8Array(task.seedSize);
  let offset = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      if (offset + value.byteLength > seed.byteLength) error(502, 'processingFailed');
      seed.set(value, offset);
      offset += value.byteLength;
    }
  } finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
  const digest = Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', seed.buffer)), (byte) => byte.toString(16).padStart(2, '0')).join('');
  if (offset !== seed.byteLength || digest !== task.seedDigest) error(502, 'processingFailed');
  return new Response(seed, { headers: {
    'Content-Type': 'application/octet-stream', 'X-Content-Type-Options': 'nosniff',
    'Content-Disposition': `attachment; filename="${file.id}.q2q"; filename*=UTF-8''${encodeURIComponent(file.name + '.q2q').replace(/['()*]/g, (char) => '%' + char.charCodeAt(0).toString(16).toUpperCase())}`
  } });
};
