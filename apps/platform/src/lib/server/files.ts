import { env } from 'cloudflare:workers';
import { error } from '@sveltejs/kit';
import { uploadMaxSize } from '#lib/upload.js';
import { requireProject } from './projects.js';
import { uploadStorage } from './upload-storage.js';

export interface FileRecord {
  id: string; projectId: string; createdBy: string; name: string; size: number;
  status: 'pending' | 'uploaded' | 'deleting' | 'deleted'; createdAt: number; updatedAt: number;
}
export async function requireFile(id: string, userId: string, write = false): Promise<FileRecord> {
  const file = await env.DB.prepare(`SELECT f.* FROM file_record f JOIN project_member m ON m.projectId = f.projectId
    WHERE f.id = ? AND m.userId = ? AND f.status <> 'deleted'
    AND (? = 0 OR f.createdBy = ? OR m.role = 'owner')`)
    .bind(id, userId, write ? 1 : 0, userId).first<FileRecord>();
  if (!file) error(404, 'fileNotFound');
  return file;
}
export async function listFiles(projectId: string, userId: string) {
  await requireProject(env.DB, userId, projectId);
  const { results } = await env.DB.prepare(`SELECT f.id, f.projectId, f.name, f.size, f.status, f.createdAt, f.updatedAt,
    (SELECT status FROM upload_task WHERE fileId = f.id ORDER BY createdAt DESC, rowid DESC LIMIT 1) AS uploadStatus,
    (SELECT status FROM seed_task WHERE fileId = f.id) AS processingStatus
    FROM file_record f WHERE f.projectId = ? AND f.status <> 'deleted' ORDER BY f.createdAt DESC LIMIT 50`)
    .bind(projectId).all<Pick<FileRecord, 'id' | 'projectId' | 'name' | 'size' | 'status' | 'createdAt' | 'updatedAt'> & { uploadStatus: import('#lib/upload.js').UploadStatus | null; processingStatus: import('../processing.js').ProcessingSummary['status'] | null }>();
  return results;
}
export async function createFile(userId: string, input: Record<string, unknown>) {
  const { id, projectId, name, size } = input;
  if (typeof id !== 'string' || !/^[a-f0-9]{8}(-[a-f0-9]{4}){3}-[a-f0-9]{12}$/i.test(id) ||
      typeof projectId !== 'string' || typeof name !== 'string' || !name.trim() || name.length > 255 ||
      /[\x00-\x1f\x7f]/.test(name) || typeof size !== 'number' || !Number.isSafeInteger(size) || size < 1 || size > uploadMaxSize) error(400, 'uploadInvalid');
  await requireProject(env.DB, userId, projectId);
  const now = Date.now();
  await env.DB.prepare(`INSERT INTO file_record (id, projectId, createdBy, name, size, status, createdAt, updatedAt)
    VALUES (?, ?, ?, ?, ?, 'pending', ?, ?) ON CONFLICT(id) DO NOTHING`)
    .bind(id, projectId, userId, name.trim(), size, now, now).run();
  const file = await requireFile(id, userId, true);
  if (file.projectId !== projectId || file.createdBy !== userId || file.name !== name.trim() || file.size !== size) error(409, 'uploadConflict');
  return file;
}
export async function deleteFile(id: string, userId: string): Promise<void> {
  const file = await env.DB.prepare(`SELECT f.* FROM file_record f JOIN project_member m ON m.projectId = f.projectId
    WHERE f.id = ? AND m.userId = ? AND (f.createdBy = ? OR m.role = 'owner')`)
    .bind(id, userId, userId).first<FileRecord>();
  if (!file) error(404, 'fileNotFound');
  if (file.status === 'deleted') return;
  // A single conditional write arbitrates deletion against new upload sessions.
  const result = await env.DB.prepare(`UPDATE file_record SET status = 'deleting', updatedAt = ?
    WHERE id = ? AND status IN ('pending', 'uploaded', 'deleting') AND NOT EXISTS (
      SELECT 1 FROM upload_task WHERE fileId = file_record.id
      AND (status IN ('initializing', 'uploading', 'completing') OR cleanupPending = 1))
    AND NOT EXISTS (SELECT 1 FROM seed_task WHERE fileId = file_record.id AND status IN ('pending', 'running'))`)
    .bind(Date.now(), id).run();
  if (!result.meta.changes) error(409, 'fileUploadActive');
  const { results } = await env.DB.prepare(`SELECT objectKey, transport, bucketIdentity FROM upload_task
    WHERE fileId = ? AND status = 'uploaded'`).bind(id).all<{ objectKey: string; transport: string; bucketIdentity: string }>();
  // No published content references exist in this stage; keep a tombstone so deletion can retry safely.
  for (const object of results) await uploadStorage(object).delete(object.objectKey);
  const artifacts = await env.DB.prepare(`SELECT s.seedKey, s.proofKey, u.transport, u.bucketIdentity FROM seed_task s
    JOIN upload_task u ON u.id = s.uploadId WHERE s.fileId = ?`).bind(id)
    .first<{ seedKey: string | null; proofKey: string | null; transport: string; bucketIdentity: string }>();
  if (artifacts) for (const key of [artifacts.seedKey, artifacts.proofKey]) if (key) await uploadStorage(artifacts).delete(key);
  await env.DB.prepare(`UPDATE file_record SET status = 'deleted', updatedAt = ? WHERE id = ? AND status = 'deleting'`).bind(Date.now(), id).run();
}
