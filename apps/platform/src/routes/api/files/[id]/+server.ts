import type { RequestHandler } from './$types';
import { uploadUser } from '#lib/server/upload-api.js';
import { deleteFile, requireFile } from '#lib/server/files.js';

export const GET: RequestHandler = async (event) => Response.json(await requireFile(event.params.id, uploadUser(event)));
export const DELETE: RequestHandler = async (event) => {
  await deleteFile(event.params.id, uploadUser(event));
  return new Response(null, { status: 204 });
};
