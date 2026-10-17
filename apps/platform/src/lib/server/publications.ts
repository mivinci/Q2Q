import { env } from 'cloudflare:workers';
import { error } from '@sveltejs/kit';
import { requireFile } from './files.js';
import type { PublicationSummary } from '../publication.js';

// Never serialize provider object keys, storage identity or pending artifact references.
const summaryColumns = `p.id, p.projectId, p.fileId, p.status, p.name, p.size, p.infoHash,
  p.createdAt, p.updatedAt, p.withdrawnAt, p.cleanupPending`;

export async function publicationBlocksDeletion(fileId: string): Promise<boolean> {
  return !!await env.DB.prepare(`SELECT id FROM content_publication WHERE fileId = ?
    AND (status IN ('creating', 'published') OR cleanupPending = 1 OR lastSignedUntil > ?) LIMIT 1`)
    .bind(fileId, Date.now()).first();
}

export async function requirePublication(id: string, userId: string, write = false): Promise<PublicationSummary> {
  const publication = await env.DB.prepare(`SELECT ${summaryColumns} FROM content_publication p
    JOIN file_record f ON f.id = p.fileId JOIN project_member m ON m.projectId = p.projectId
    WHERE p.id = ? AND m.userId = ? AND (? = 0 OR f.createdBy = ? OR m.role = 'owner')`)
    .bind(id, userId, write ? 1 : 0, userId).first<PublicationSummary>();
  if (!publication) error(404, 'publicationNotFound');
  return publication;
}

export async function listPublications(fileId: string, userId: string): Promise<PublicationSummary[]> {
  await requireFile(fileId, userId);
  const { results } = await env.DB.prepare(`SELECT ${summaryColumns} FROM content_publication p
    JOIN project_member m ON m.projectId = p.projectId WHERE p.fileId = ? AND m.userId = ?
    ORDER BY p.createdAt DESC, p.id LIMIT 50`).bind(fileId, userId).all<PublicationSummary>();
  return results;
}

export async function listProjectPublications(projectId: string, userId: string): Promise<PublicationSummary[]> {
  const { results } = await env.DB.prepare(`SELECT ${summaryColumns} FROM content_publication p
    JOIN project_member m ON m.projectId = p.projectId WHERE p.projectId = ? AND m.userId = ?
    ORDER BY p.createdAt DESC, p.id LIMIT 100`).bind(projectId, userId).all<PublicationSummary>();
  return results;
}

export async function createPublication(fileId: string, userId: string, input: Record<string, unknown>) {
  if (typeof input.id !== 'string' || !/^[a-f0-9]{8}(-[a-f0-9]{4}){3}-[a-f0-9]{12}$/i.test(input.id)) error(400, 'publicationInvalid');
  const file = await requireFile(fileId, userId, true);
  const existing = await env.DB.prepare('SELECT fileId FROM content_publication WHERE id = ?').bind(input.id).first<{ fileId: string }>();
  if (existing) {
    if (existing.fileId !== fileId) error(409, 'publicationConflict');
    await dispatchPublication(input.id, fileId);
    return requirePublication(input.id, userId, true);
  }
  const now = Date.now();
  // This one write arbitrates against file deletion and concurrent publication creation.
  // It freezes a ready processing result before the queue copies independent artifacts.
  await env.DB.prepare(`INSERT INTO content_publication (
    id, projectId, fileId, uploadId, createdBy, status, name, size, infoHash,
    objectKey, objectVersion, objectEtag, transport, bucketIdentity,
    preparedSeedKey, preparedProofKey, seedDigest, seedSize, createdAt, updatedAt)
    SELECT ?, f.projectId, f.id, u.id, ?, 'creating', f.name, f.size, s.infoHash,
      u.objectKey, u.objectVersion, u.objectEtag, u.transport, u.bucketIdentity,
      s.seedKey, s.proofKey, s.seedDigest, s.seedSize, ?, ?
    FROM file_record f JOIN seed_task s ON s.fileId = f.id JOIN upload_task u ON u.id = s.uploadId
    JOIN project_member m ON m.projectId = f.projectId AND m.userId = ?
    WHERE f.id = ? AND f.status = 'uploaded' AND s.status = 'ready' AND u.status = 'uploaded'
      AND (f.createdBy = ? OR m.role = 'owner') AND u.objectEtag IS NOT NULL
      AND s.infoHash IS NOT NULL AND s.seedKey IS NOT NULL AND s.proofKey IS NOT NULL
      AND s.seedDigest IS NOT NULL AND s.seedSize > 0
      AND NOT EXISTS (SELECT 1 FROM content_publication WHERE fileId = f.id AND status IN ('creating', 'published'))
    ON CONFLICT DO NOTHING`).bind(input.id, userId, now, now, userId, fileId, userId).run();
  const inserted = await env.DB.prepare('SELECT fileId FROM content_publication WHERE id = ?').bind(input.id).first<{ fileId: string }>();
  if (inserted) {
    if (inserted.fileId !== fileId) error(409, 'publicationConflict');
    await dispatchPublication(input.id, fileId);
    return requirePublication(input.id, userId, true);
  }
  await requireFile(fileId, userId, true);
  const active = await env.DB.prepare(`SELECT id FROM content_publication WHERE fileId = ? AND status IN ('creating', 'published')`)
    .bind(fileId).first();
  error(409, active ? 'publicationActive' : 'publicationNotReady');
}

async function dispatchPublication(id: string, fileId: string) {
  try {
    if (!env.PROCESSING_QUEUE) throw new Error('processingUnavailable');
    await env.PROCESSING_QUEUE.send({ kind: 'publication', publicationId: id, fileId });
  } catch {
    await env.DB.prepare(`UPDATE content_publication SET status = 'failed', errorCode = 'processingUnavailable', updatedAt = ?
      WHERE id = ? AND status = 'creating' AND leaseUntil = 0`).bind(Date.now(), id).run();
  }
}

export async function withdrawPublication(id: string, userId: string) {
  const publication = await requirePublication(id, userId, true);
  const now = Date.now();
  // Withdrawal is terminal. Later re-publication uses a new ID and cannot revive old grants.
  await env.DB.prepare(`UPDATE content_publication SET status = 'withdrawn', withdrawnAt = ?, updatedAt = ?,
    cleanupPending = CASE WHEN seedKey IS NOT NULL OR cleanupPending = 1 THEN 1 ELSE 0 END
    WHERE id = ? AND status <> 'withdrawn' AND EXISTS (
      SELECT 1 FROM file_record f JOIN project_member m ON m.projectId = f.projectId
      WHERE f.id = content_publication.fileId AND m.userId = ? AND (f.createdBy = ? OR m.role = 'owner'))`)
    .bind(now, now, id, userId, userId).run();
  await env.PROCESSING_QUEUE?.send({ kind: 'publication-cleanup', publicationId: id, fileId: publication.fileId }).catch(() => {});
  return requirePublication(id, userId, true);
}
