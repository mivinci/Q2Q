import type { ProcessingSummary } from '../processing.js';
import type { SeedProcessingMessage } from '@q2q/seed-processing-contract';

export async function processingSummary(db: D1Database, fileId: string) {
  return db.prepare(`SELECT status, processedBytes, errorCode, infoHash, leaseUntil, updatedAt FROM seed_task WHERE fileId = ?`)
    .bind(fileId).first<ProcessingSummary>();
}

export async function enqueueSeed(db: D1Database, queue: Queue | undefined, fileId: string) {
  const now = Date.now();
  await db.prepare(`INSERT INTO seed_task (fileId, uploadId, status, createdAt, updatedAt)
    SELECT f.id, u.id, 'pending', ?, ? FROM file_record f JOIN upload_task u ON u.fileId = f.id
    WHERE f.id = ? AND f.status = 'uploaded' AND u.status = 'uploaded'
    ON CONFLICT(fileId) DO NOTHING`).bind(now, now, fileId).run();
  // D1 is the durable intent. Retrying dispatch may duplicate messages; the consumer fences execution.
  try {
    if (!queue) throw new Error('processingUnavailable');
    await queue.send({ fileId } satisfies SeedProcessingMessage);
  } catch {
    await db.prepare(`UPDATE seed_task SET status = 'failed', errorCode = 'processingUnavailable', updatedAt = ? WHERE fileId = ? AND status = 'pending'`)
      .bind(Date.now(), fileId).run();
    throw new Error('processingUnavailable');
  }
}
