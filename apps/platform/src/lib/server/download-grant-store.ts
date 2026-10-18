import { error } from '@sveltejs/kit';
import { createS3Storage, signS3Download, type S3Config } from '@q2q/s3-storage';

const digest = async (value: string) => Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(value))), (byte) => byte.toString(16).padStart(2, '0')).join('');
const columns = 'id, publicationId, createdAt, revokedAt';

async function requireManager(db: D1Database, id: string, userId: string) {
  const row = await db.prepare(`SELECT p.id FROM content_publication p JOIN file_record f ON f.id = p.fileId
    JOIN project_member m ON m.projectId = p.projectId WHERE p.id = ? AND m.userId = ?
    AND (f.createdBy = ? OR m.role = 'owner')`).bind(id, userId, userId).first();
  if (!row) error(404, 'publicationNotFound');
}

export async function listDownloadGrants(db: D1Database, id: string, userId: string) {
  await requireManager(db, id, userId);
  return (await db.prepare(`SELECT ${columns} FROM download_grant WHERE publicationId = ? ORDER BY createdAt DESC LIMIT 100`).bind(id).all()).results;
}

export async function createDownloadGrant(db: D1Database, id: string, userId: string, origin: string) {
  await requireManager(db, id, userId);
  const secret = Array.from(crypto.getRandomValues(new Uint8Array(32)), (byte) => byte.toString(16).padStart(2, '0')).join('');
  const grantId = crypto.randomUUID();
  const now = Date.now();
  // Recheck both membership and publication state in the same write that creates access.
  const inserted = await db.prepare(`INSERT INTO download_grant (id, publicationId, secretDigest, createdBy, createdAt)
    SELECT ?, p.id, ?, ?, ? FROM content_publication p JOIN file_record f ON f.id = p.fileId
    JOIN project_member m ON m.projectId = p.projectId WHERE p.id = ? AND p.status = 'published'
    AND m.userId = ? AND (f.createdBy = ? OR m.role = 'owner') RETURNING id`)
    .bind(grantId, await digest(secret), userId, now, id, userId, userId).first();
  if (!inserted) error(409, 'publicationNotReady');
  const source = new URL(`/api/publications/${id}/content`, origin);
  source.searchParams.set('grant', grantId);
  source.searchParams.set('secret', secret);
  // The original bearer is returned once. Lists and subsequent reads cannot recover it.
  return { id: grantId, publicationId: id, createdAt: now, sourceUrl: source.toString() };
}

export async function revokeDownloadGrant(db: D1Database, id: string, grantId: string, userId: string) {
  await requireManager(db, id, userId);
  const result = await db.prepare(`UPDATE download_grant SET revokedAt = COALESCE(revokedAt, ?)
    WHERE id = ? AND publicationId = ? AND EXISTS (
      SELECT 1 FROM content_publication p JOIN file_record f ON f.id = p.fileId
      JOIN project_member m ON m.projectId = p.projectId WHERE p.id = download_grant.publicationId
      AND m.userId = ? AND (f.createdBy = ? OR m.role = 'owner')) RETURNING id`)
    .bind(Date.now(), grantId, id, userId, userId).first();
  if (!result) error(404, 'downloadGrantNotFound');
}

export async function downloadLocation(db: D1Database, config: S3Config, id: string, grantId: string, secret: string, range: string | null) {
  if (!/^[a-f0-9]{64}$/.test(secret) || !/^[a-f0-9-]{36}$/.test(grantId)) error(404, 'downloadGrantNotFound');
  const now = Date.now();
  // Reserving the maximum link lifetime arbitrates with withdrawal and file deletion.
  // No signed URL is returned unless this write observes a live version and grant.
  const row = await db.prepare(`UPDATE content_publication SET lastSignedUntil = MAX(lastSignedUntil, ?)
    WHERE id = ? AND status = 'published' AND EXISTS (
      SELECT 1 FROM download_grant g WHERE g.publicationId = content_publication.id
      AND g.id = ? AND g.secretDigest = ? AND g.revokedAt IS NULL)
    RETURNING objectKey, objectVersion, objectEtag, size, transport, bucketIdentity`)
    .bind(now + 300_000, id, grantId, await digest(secret))
    .first<{ objectKey: string; objectVersion: string | null; objectEtag: string; size: number; transport: string; bucketIdentity: string }>();
  if (!row) error(404, 'downloadGrantNotFound');
  if (range) {
    const match = /^bytes=(\d*)-(\d*)$/.exec(range);
    const start = match?.[1] ? Number(match[1]) : null;
    const end = match?.[2] ? Number(match[2]) : null;
    if (!match || (start === null && end === null) ||
      (start !== null && (!Number.isSafeInteger(start) || start >= row.size)) ||
      (end !== null && (!Number.isSafeInteger(end) || (start === null ? end <= 0 : end < start)))) error(416, 'downloadRangeInvalid');
  }
  const identity = JSON.stringify({ endpoint: new URL(config.endpoint).toString().replace(/\/$/, ''), region: config.region, bucket: config.bucket, forcePathStyle: config.forcePathStyle !== false });
  if (row.transport !== 's3' || row.bucketIdentity !== identity) error(503, 'downloadUnavailable');
  const storage = createS3Storage(config);
  const object = await storage.get(row.objectKey, { etag: row.objectEtag, version: row.objectVersion });
  await object.body?.cancel();
  if (!object.ok || Number(object.headers.get('content-length')) !== row.size) error(409, 'publicationSourceChanged');
  const location = await signS3Download(config, row.objectKey, row.objectVersion);
  // Signing must finish within the reservation's safety margin, including cold starts.
  if (Date.now() > now + 55_000) error(503, 'downloadUnavailable');
  return location;
}
