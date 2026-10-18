import type { RequestHandler } from './$types';
import { env } from 'cloudflare:workers';
import { enqueueSeed } from '#lib/server/seed-tasks.js';
import { uploadJson, uploadUser } from '#lib/server/upload-api.js';
import { completeUpload, requireUpload } from '#lib/server/uploads.js';

export const POST: RequestHandler = async (event) => {
  const userId = uploadUser(event);
  const input = await uploadJson(event.request);
  const task = await requireUpload(event.params.id, userId);
  await completeUpload(task, input.parts);
  // Upload completion stays successful if queue dispatch fails; D1 retains retryable intent.
  if (task.transport === 's3') await enqueueSeed(env.DB, env.PROCESSING_QUEUE, task.fileId).catch(() => {});
  return Response.json({ status: 'uploaded' });
};
