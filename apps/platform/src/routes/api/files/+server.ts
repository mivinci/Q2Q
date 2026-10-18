import type { RequestHandler } from './$types';
import { uploadJson, uploadUser } from '#lib/server/upload-api.js';
import { createFile, listFiles } from '#lib/server/files.js';

export const POST: RequestHandler = async (event) => Response.json(
  await createFile(uploadUser(event), await uploadJson(event.request)), { status: 201 }
);
export const GET: RequestHandler = async (event) => Response.json(
  await listFiles(event.url.searchParams.get('projectId') ?? '', uploadUser(event))
);
