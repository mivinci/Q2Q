import type { RequestHandler } from './$types';
import { uploadUser, uploadJson } from '#lib/server/upload-api.js';
import { createPublication, listPublications } from '#lib/server/publications.js';

export const GET: RequestHandler = async (event) => Response.json(await listPublications(event.params.id, uploadUser(event)));
export const POST: RequestHandler = async (event) => {
  const publication = await createPublication(event.params.id, uploadUser(event), await uploadJson(event.request));
  return Response.json(publication, { status: publication.status === 'creating' ? 202 : 200 });
};
