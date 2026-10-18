import { env } from 'cloudflare:workers';
import { s3Config } from './upload-storage.js';
import * as store from './download-grant-store.js';

export const listDownloadGrants = (id: string, userId: string) => store.listDownloadGrants(env.DB, id, userId);
export const createDownloadGrant = async (id: string, userId: string, origin: string) => {
  const grant = await store.createDownloadGrant(env.DB, id, userId, origin);
  const version = await env.DB.prepare('SELECT trackUrl FROM content_publication WHERE id = ?').bind(id).first<{ trackUrl: string | null }>();
  return { ...grant, trackUrl: version?.trackUrl ?? null };
};
export const revokeDownloadGrant = (id: string, grantId: string, userId: string) => store.revokeDownloadGrant(env.DB, id, grantId, userId);
export const downloadLocation = (id: string, grantId: string, secret: string, range: string | null) => store.downloadLocation(env.DB, s3Config(), id, grantId, secret, range);
