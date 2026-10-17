import { env } from 'cloudflare:workers';
import { s3Config } from './upload-storage.js';
import * as store from './download-grant-store.js';

export const listDownloadGrants = (id: string, userId: string) => store.listDownloadGrants(env.DB, id, userId);
export const createDownloadGrant = (id: string, userId: string, origin: string) => store.createDownloadGrant(env.DB, id, userId, origin);
export const revokeDownloadGrant = (id: string, grantId: string, userId: string) => store.revokeDownloadGrant(env.DB, id, grantId, userId);
export const downloadLocation = (id: string, grantId: string, secret: string, range: string | null) => store.downloadLocation(env.DB, s3Config(), id, grantId, secret, range);
