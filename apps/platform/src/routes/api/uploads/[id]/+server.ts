import { error } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import { uploadUser } from '#lib/server/upload-api.js';
import { cancelUpload, partLength, requireUpload, requireWritable } from '#lib/server/uploads.js';
import { readBoundedBody, storageBucket, uploadTransport, uploadStorage } from '#lib/server/upload-storage.js';

export const PUT: RequestHandler = async (event) => {
  const userId = uploadUser(event);
  if (uploadTransport() !== 'local') error(404, 'uploadNotFound');
  const task = await requireUpload(event.params.id, userId);
  requireWritable(task);
  if (task.transport !== 'local') error(404, 'uploadNotFound');
  uploadStorage(task);
  const partNumber = Number(event.url.searchParams.get('partNumber'));
  const size = partLength(task, partNumber);
  // Only the explicit development adapter accepts bytes through the app, bounded to one part.
  const body = await readBoundedBody(event.request, size);
  const receipt = await storageBucket().resumeMultipartUpload(task.objectKey, task.multipartId!).uploadPart(partNumber, body);
  return new Response(null, { headers: { ETag: `"${receipt.etag}"` } });
};

export const DELETE: RequestHandler = async (event) => {
  const userId = uploadUser(event);
  await cancelUpload(await requireUpload(event.params.id, userId));
  const task = await requireUpload(event.params.id, userId);
  return Response.json({ status: task.status, cleanupPending: !!task.cleanupPending });
};
