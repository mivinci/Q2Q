import { env } from 'cloudflare:workers';
import { error } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import { requireFile } from '#lib/server/files.js';
import { uploadUser } from '#lib/server/upload-api.js';
import { enqueueSeed, processingSummary } from '#lib/server/seed-tasks.js';

export const GET: RequestHandler = async (event) => {
  await requireFile(event.params.id, uploadUser(event));
  return Response.json(await processingSummary(env.DB, event.params.id));
};

export const POST: RequestHandler = async (event) => {
  const file = await requireFile(event.params.id, uploadUser(event), true);
  if (file.status !== 'uploaded') error(409, 'uploadConflict');
  const source = await env.DB.prepare(`SELECT transport FROM upload_task WHERE fileId = ? AND status = 'uploaded' LIMIT 1`)
    .bind(file.id).first<{ transport: string }>();
  if (source?.transport !== 's3') error(503, 'processingUnavailable');
  const task = await processingSummary(env.DB, file.id);
  if (task?.status === 'ready' || (task?.status === 'running' && (task.leaseUntil ?? 0) > Date.now())) return Response.json(task);
  await env.DB.prepare(`UPDATE seed_task SET status = 'pending', updatedAt = ? WHERE fileId = ? AND
    (status IN ('failed', 'pending') OR (status = 'running' AND leaseUntil < ?))`).bind(Date.now(), file.id, Date.now()).run();
  try { await enqueueSeed(env.DB, env.PROCESSING_QUEUE, file.id); }
  catch { error(503, 'processingUnavailable'); }
  return Response.json(await processingSummary(env.DB, file.id), { status: 202 });
};
