import { env } from 'cloudflare:workers';
import { error } from '@sveltejs/kit';
import { uploadPartSize, type UploadSummary, type UploadedPart } from '#lib/upload.js';
import { requireFile } from './files.js';
import { getPlatformStorage } from './storage.js';
import { uploadStorage, storageIdentity, uploadTransport } from './upload-storage.js';

interface UploadTask extends UploadSummary {
  fileId: string; transport: string; bucketIdentity: string;
  projectId: string; connectionId: string; createdBy: string; objectKey: string;
  multipartId: string | null; partSize: number; expiresAt: number; partsJson: string | null;
}

export async function listUploads(projectId: string, userId: string, fileId: string | null = null): Promise<UploadSummary[]> {
  const { results } = await env.DB.prepare(`SELECT t.id, t.fileId, t.name, t.size, t.status, t.cleanupPending, t.createdAt, t.updatedAt,
    CASE WHEN t.createdBy = ? OR m.role = 'owner' THEN 1 ELSE 0 END AS canManage
    FROM upload_task t JOIN project_member m ON m.projectId = t.projectId
    JOIN file_record f ON f.id = t.fileId
    WHERE t.projectId = ? AND m.userId = ? AND f.status <> 'deleted' AND (? IS NULL OR t.fileId = ?) ORDER BY t.createdAt DESC, t.rowid DESC LIMIT 50`).bind(userId, projectId, userId, fileId, fileId).all<UploadSummary>();
  return results;
}

export async function requireUpload(id: string, userId: string): Promise<UploadTask> {
  const task = await env.DB.prepare(`SELECT t.*, 1 AS canManage FROM upload_task t JOIN project_member m ON m.projectId = t.projectId
    JOIN file_record f ON f.id = t.fileId AND f.status IN ('pending', 'uploaded')
    WHERE t.id = ? AND m.userId = ? AND (t.createdBy = ? OR m.role = 'owner')`)
    .bind(id, userId, userId).first<UploadTask>();
  if (!task) error(404, 'uploadNotFound');
  return task;
}

export function partLength(task: UploadTask, partNumber: number): number {
  const count = Math.ceil(task.size / task.partSize);
  if (!Number.isInteger(partNumber) || partNumber < 1 || partNumber > count) error(400, 'uploadInvalid');
  return Math.min(task.partSize, task.size - (partNumber - 1) * task.partSize);
}

export function requireWritable(task: UploadTask) {
  if (task.status !== 'uploading') error(409, 'uploadConflict');
  if (task.expiresAt <= Date.now()) error(410, 'uploadExpired');
}

export async function createUpload(userId: string, fileId: string, input: Record<string, unknown>) {
  const { id } = input;
  if (typeof id !== 'string' || !/^[a-f0-9]{8}(-[a-f0-9]{4}){3}-[a-f0-9]{12}$/i.test(id)) error(400, 'uploadInvalid');
  const file = await requireFile(fileId, userId, true);
  const { projectId, name, size } = file;
  const mode = uploadTransport();
  if (!mode) error(503, 'uploadUnavailable');
  if (mode === 'local' && size > 64 * 1024 * 1024) error(413, 'uploadLocalLimit');
  const connection = await getPlatformStorage(env.DB, projectId);
  if (!connection) error(409, 'uploadStorageRequired');
  const existing = await env.DB.prepare('SELECT id FROM upload_task WHERE id = ?').bind(id).first();
  if (existing) {
    const task = await requireUpload(id, userId);
    if (task.fileId !== fileId || task.createdBy !== userId) error(409, 'uploadConflict');
    return { id: task.id, partSize: task.partSize, status: task.status };
  }
  if (file.status !== 'pending') error(409, 'uploadConflict');
  const now = Date.now();
  const key = `projects/${encodeURIComponent(projectId)}/uploads/${id}/object`;
  // Persist intent before creating provider state. A stranded initializing task is visible for cleanup.
  const inserted = await env.DB.prepare(`INSERT INTO upload_task
    (id, projectId, connectionId, createdBy, name, size, objectKey, partSize, status, createdAt, updatedAt, expiresAt, transport, bucketIdentity, fileId)
    SELECT ?, ?, ?, ?, ?, ?, ?, ?, 'initializing', ?, ?, ?, ?, ?, ?
    FROM file_record f WHERE f.id = ? AND f.status = 'pending' AND NOT EXISTS
    (SELECT 1 FROM upload_task WHERE fileId = f.id AND status IN ('initializing', 'uploading', 'completing'))
    ON CONFLICT(id) DO NOTHING`)
    .bind(id, projectId, connection.id, userId, name.trim(), size, key, uploadPartSize, now, now, now + 86400000, mode, storageIdentity(mode), fileId, fileId).run();
  if (!inserted.meta.changes) {
    const found = await env.DB.prepare('SELECT id FROM upload_task WHERE id = ?').bind(id).first();
    if (!found) error(409, 'uploadConflict');
    const task = await requireUpload(id, userId);
    if (task.fileId !== fileId || task.createdBy !== userId) error(409, 'uploadConflict');
    return { id: task.id, partSize: task.partSize, status: task.status };
  }
  try {
    const multipart = await uploadStorage().createMultipartUpload(key, id);
    try {
      await env.DB.prepare(`UPDATE upload_task SET multipartId = ?, status = 'uploading', updatedAt = ? WHERE id = ? AND status = 'initializing'`)
        .bind(multipart.uploadId, Date.now(), id).run();
    } catch (cause) {
      await multipart.abort().catch(() => {});
      throw cause;
    }
  } catch (cause) {
    // Keep provider credentials and request URLs out of diagnostics.
    console.error('Upload initialization failed', {
      type: cause instanceof Error ? cause.name : 'unknown',
      status: cause instanceof Error ? /^S3 initialization failed \((\d+)\)$/.exec(cause.message)?.[1] : undefined,
      frame: cause instanceof Error ? cause.stack?.split('\n')[1]?.split('?')[0] : undefined
    });
    await env.DB.prepare(`UPDATE upload_task SET status = 'failed', updatedAt = ? WHERE id = ? AND status = 'initializing'`).bind(Date.now(), id).run();
    error(502, 'uploadFailed');
  }
  return { id, partSize: uploadPartSize, status: 'uploading' as const };
}

function validateParts(task: UploadTask, value: unknown): UploadedPart[] {
  if (!Array.isArray(value) || value.length !== Math.ceil(task.size / task.partSize)) error(400, 'uploadInvalid');
  return value.map((part: unknown, index) => {
    if (!part || typeof part !== 'object' || !('partNumber' in part) || !('etag' in part) ||
        part.partNumber !== index + 1 || typeof part.etag !== 'string' || !part.etag || part.etag.length > 128 || /[\x00-\x1f]/.test(part.etag)) error(400, 'uploadInvalid');
    return { partNumber: index + 1, etag: part.etag.replace(/^"|"$/g, '') };
  });
}

export async function completeUpload(task: UploadTask, value: unknown) {
  if (task.status === 'uploaded') return;
  if (task.status === 'uploading') {
    requireWritable(task);
    const parts = validateParts(task, value);
    // Completion wins a CAS against cancellation and freezes the exact manifest for all retries.
    await env.DB.prepare(`UPDATE upload_task SET status = 'completing', partsJson = ?, updatedAt = ? WHERE id = ? AND status = 'uploading'`)
      .bind(JSON.stringify(parts), Date.now(), task.id).run();
    const fresh = await env.DB.prepare('SELECT * FROM upload_task WHERE id = ?').bind(task.id).first<UploadTask>();
    if (!fresh) error(404, 'uploadNotFound');
    task = fresh;
  }
  if (task.status === 'uploaded') return;
  if (task.status !== 'completing' || !task.multipartId || !task.partsJson) error(409, 'uploadConflict');
  const bucket = uploadStorage(task);
  // Recover a successful R2 completion whose D1 update or HTTP response was lost.
  let object = await bucket.head(task.objectKey);
  if (!object) {
    try {
      object = await bucket.resumeMultipartUpload(task.objectKey, task.multipartId).complete(JSON.parse(task.partsJson) as UploadedPart[]);
    } catch {
      object = await bucket.head(task.objectKey);
      if (!object) error(502, 'uploadCompleteRetry');
    }
  }
  if (object.size !== task.size || object.customMetadata?.uploadTask !== task.id) {
    await env.DB.prepare(`UPDATE upload_task SET status = 'failed', cleanupPending = 1, updatedAt = ? WHERE id = ? AND status = 'completing'`).bind(Date.now(), task.id).run();
    error(422, 'uploadSizeMismatch');
  }
  const now = Date.now();
  await env.DB.batch([
    env.DB.prepare(`UPDATE upload_task SET status = 'uploaded', objectVersion = ?, objectEtag = ?, updatedAt = ?
      WHERE id = ? AND status = 'completing'`).bind(object.version, object.etag, now, task.id),
    env.DB.prepare(`UPDATE file_record SET status = 'uploaded', updatedAt = ? WHERE id = ? AND status = 'pending'
      AND EXISTS (SELECT 1 FROM upload_task WHERE id = ? AND status = 'uploaded')`).bind(now, task.fileId, task.id)
  ]);
}

export async function cancelUpload(task: UploadTask) {
  if (task.status === 'completing' || task.status === 'uploaded' || task.status === 'initializing') error(409, 'uploadConflict');
  await env.DB.prepare(`UPDATE upload_task SET status = 'cancelled', cleanupPending = 1, updatedAt = ? WHERE id = ? AND (status = 'uploading' OR (status = 'failed' AND cleanupPending = 1))`).bind(Date.now(), task.id).run();
  const fresh = await env.DB.prepare('SELECT * FROM upload_task WHERE id = ?').bind(task.id).first<UploadTask>();
  if (!fresh || fresh.status !== 'cancelled') error(409, 'uploadConflict');
  if (fresh.multipartId && fresh.cleanupPending) {
    try {
      const bucket = uploadStorage(fresh);
      const object = await bucket.head(fresh.objectKey);
      // A failed completion can leave an unpublished object, rather than open multipart state.
      if (object) await bucket.delete(fresh.objectKey);
      else await bucket.resumeMultipartUpload(fresh.objectKey, fresh.multipartId).abort();
      await env.DB.prepare(`UPDATE upload_task SET cleanupPending = 0, updatedAt = ? WHERE id = ? AND status = 'cancelled'`).bind(Date.now(), task.id).run();
    } catch {
      // Logical cancellation is durable. A failed provider cleanup remains visible and retryable.
    }
  }
}
