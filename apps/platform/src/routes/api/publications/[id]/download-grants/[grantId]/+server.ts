import type { RequestHandler } from './$types';
import { uploadUser } from '#lib/server/upload-api.js';
import { revokeDownloadGrant } from '#lib/server/download-grants.js';
export const DELETE: RequestHandler = async (event) => {
  await revokeDownloadGrant(event.params.id, event.params.grantId, uploadUser(event));
  return Response.json({ revoked: true });
};
