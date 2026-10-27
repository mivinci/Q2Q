import { effectivePlanIdSql } from '@q2q/plans';
import { uploadPartSize } from '../upload.js';

export interface StorageUsage { usedBytes: number; reservedBytes: number }

// Task state is the reservation ledger: completion transfers reserved to used;
// failed cleanup and file tombstones retain space until provider deletion succeeds.
const occupied = `FROM upload_task t
  JOIN file_record qf ON qf.id = t.fileId
  JOIN storage_connection sc ON sc.id = t.connectionId
  WHERE t.projectId = ? AND sc.provider = 'platform-s3' AND qf.status <> 'deleted'
    AND (t.status IN ('initializing', 'uploading', 'completing', 'uploaded') OR t.cleanupPending = 1)`;

export async function getStorageUsage(db: D1Database, projectId: string): Promise<StorageUsage> {
  return await db.prepare(`SELECT
    COALESCE(SUM(CASE WHEN t.status = 'uploaded' THEN t.size ELSE 0 END), 0) AS usedBytes,
    COALESCE(SUM(CASE WHEN t.status <> 'uploaded' THEN t.size ELSE 0 END), 0) AS reservedBytes
    ${occupied}`).bind(projectId).first<StorageUsage>() ?? { usedBytes: 0, reservedBytes: 0 };
}

// Evaluate subscription and usage in the INSERT, not in a preceding read. D1's
// serialized writes arbitrate concurrent uploads and a simultaneous downgrade.
function limitSql(key: 'storageBytes' | 'maxFileSizeBytes' | 'platformStorage') {
  return `(SELECT catalog.${key} FROM plan_catalog catalog WHERE catalog.id =
    ${effectivePlanIdSql('f.projectId', '(SELECT now FROM request_time)')})`;
}

export async function reserveUpload(db: D1Database, input: {
  id: string; fileId: string; userId: string; connectionId: string;
  objectKey: string; transport: string; bucketIdentity: string; now: number;
}) {
  return db.prepare(`WITH request_time(now) AS (VALUES (?))
    INSERT INTO upload_task
    (id, projectId, connectionId, createdBy, name, size, objectKey, partSize, status,
      createdAt, updatedAt, expiresAt, transport, bucketIdentity, fileId)
    SELECT ?, f.projectId, c.id, ?, f.name, f.size, ?, ?, 'initializing', ?, ?, ?, ?, ?, f.id
    FROM file_record f JOIN storage_connection c ON c.projectId = f.projectId AND c.id = ? AND c.provider = 'platform-s3'
    JOIN project_member m ON m.projectId = f.projectId AND m.userId = ?
    WHERE f.id = ? AND f.status = 'pending' AND (f.createdBy = ? OR m.role = 'owner')
      AND NOT EXISTS (SELECT 1 FROM upload_task WHERE fileId = f.id AND status IN ('initializing', 'uploading', 'completing'))
      AND ${limitSql('platformStorage')} = 1
      AND f.size <= ${limitSql('maxFileSizeBytes')}
      AND COALESCE((SELECT SUM(t.size) ${occupied.replace('t.projectId = ?', 't.projectId = f.projectId')}), 0)
        <= ${limitSql('storageBytes')} - f.size
    ON CONFLICT(id) DO NOTHING`)
    .bind(input.now, input.id, input.userId, input.objectKey, uploadPartSize, input.now, input.now, input.now + 86400000,
      input.transport, input.bucketIdentity, input.connectionId, input.userId, input.fileId, input.userId).run();
}
