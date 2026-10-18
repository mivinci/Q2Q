import type { RequestHandler } from './$types';
import { uploadJson, uploadUser } from '#lib/server/upload-api.js';
import { createUpload } from '#lib/server/uploads.js';

export const POST: RequestHandler = async (event) => Response.json(
  await createUpload(uploadUser(event), event.params.id, await uploadJson(event.request)), { status: 201 }
);
