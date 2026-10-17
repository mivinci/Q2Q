import { error } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import { uploadJson, uploadUser } from '#lib/server/upload-api.js';
import { partLength, requireUpload, requireWritable } from '#lib/server/uploads.js';
import { partGrant } from '#lib/server/upload-storage.js';

export const POST: RequestHandler = async (event) => {
  const userId = uploadUser(event);
  const input = await uploadJson(event.request);
  const task = await requireUpload(event.params.id, userId);
  requireWritable(task);
  if (typeof input.partNumber !== 'number') error(400, 'uploadInvalid');
  return Response.json(await partGrant(task, input.partNumber, partLength(task, input.partNumber)));
};
