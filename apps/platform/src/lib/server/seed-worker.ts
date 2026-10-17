import { createS3Storage, type S3Config } from './s3-storage.js';
import type { ProcessingMessage } from '../processing.js';

interface Source {
  fileId: string; uploadId: string; name: string; size: number;
  objectKey: string; objectEtag: string; objectVersion: string | null;
  transport: string; bucketIdentity: string;
}
interface Task {
  fileId: string; uploadId: string; leaseToken: string; seedKey: string | null; proofKey: string | null;
}
export interface SeedComputer {
  update(bytes: Uint8Array): void;
  finish_inline(name: string): { seed_bytes(): Uint8Array; proof_bytes(): Uint8Array; info_hash(): Uint8Array; free(): void };
  free(): void;
}
const leaseMs = 120_000;

export async function processSeed(db: D1Database, config: S3Config, message: ProcessingMessage, create: (size: number) => SeedComputer) {
  const now = Date.now();
  const token = crypto.randomUUID();
  const task = await db.prepare(`UPDATE seed_task SET status = 'running', leaseToken = ?, leaseUntil = ?,
    attempt = attempt + 1, processedBytes = 0, errorCode = NULL, updatedAt = ?
    WHERE fileId = ? AND (status IN ('pending', 'failed') OR (status = 'running' AND leaseUntil < ?))
    AND EXISTS (SELECT 1 FROM file_record f WHERE f.id = seed_task.fileId AND f.status = 'uploaded')
    RETURNING fileId, uploadId, leaseToken, seedKey, proofKey`)
    .bind(token, now + leaseMs, now, message.fileId, now).first<Task>();
  if (!task) {
    const state = await db.prepare('SELECT status FROM seed_task WHERE fileId = ?').bind(message.fileId).first<{ status: string }>();
    return state?.status === 'running' ? 'busy' : 'done';
  }
  let computer: SeedComputer | undefined;
  let consumed = false;
  let storage: ReturnType<typeof createS3Storage> | undefined;
  let seedKey: string | undefined;
  let proofKey: string | undefined;
  let reader: ReadableStreamDefaultReader<Uint8Array> | undefined;
  try {
    const source = await db.prepare(`SELECT u.*, f.name, f.size FROM upload_task u JOIN file_record f ON f.id = u.fileId
      WHERE u.id = ? AND f.id = ? AND u.status = 'uploaded' AND f.status = 'uploaded'`)
      .bind(task.uploadId, task.fileId).first<Source>();
    const identity = JSON.stringify({ endpoint: new URL(config.endpoint).toString().replace(/\/$/, ''), region: config.region, bucket: config.bucket, forcePathStyle: config.forcePathStyle });
    if (!source || source.transport !== 's3' || source.bucketIdentity !== identity || !source.objectEtag) throw new Error('processingSourceChanged');
    storage = createS3Storage(config);
    // Attempt keys are never reused, including after lease expiry or a lost acknowledgement.
    const prefix = `projects/processing/${task.fileId}/${token}`;
    seedKey = `${prefix}/seed.q2q`;
    proofKey = `${prefix}/proof.bin`;
    for (const key of [task.seedKey, task.proofKey]) if (key) await storage.delete(key);
    const intent = await db.prepare(`UPDATE seed_task SET seedKey = ?, proofKey = ? WHERE fileId = ? AND leaseToken = ? AND status = 'running'`)
      .bind(seedKey, proofKey, task.fileId, token).run();
    if (!intent.meta.changes) throw new Error('processingLeaseLost');
    const response = await storage.get(source.objectKey, { etag: source.objectEtag, version: source.objectVersion });
    if (!response.body || Number(response.headers.get('content-length')) !== source.size) {
      await response.body?.cancel(); throw new Error('processingSourceChanged');
    }
    computer = create(source.size);
    reader = response.body.getReader();
    let received = 0;
    let heartbeat = Date.now();
    while (true) {
      const chunk = await reader.read();
      if (chunk.done) break;
      computer.update(chunk.value);
      received += chunk.value.byteLength;
      if (Date.now() - heartbeat > 10_000) {
        heartbeat = Date.now();
        const renewed = await db.prepare(`UPDATE seed_task SET leaseUntil = ?, processedBytes = ?, updatedAt = ?
          WHERE fileId = ? AND leaseToken = ? AND status = 'running' AND leaseUntil > ?`)
          .bind(heartbeat + leaseMs, received, heartbeat, task.fileId, token, heartbeat).run();
        if (!renewed.meta.changes) throw new Error('processingLeaseLost');
      }
    }
    if (received !== source.size) throw new Error('processingSourceChanged');
    consumed = true;
    const result = computer.finish_inline(source.name);
    try {
      const seed = result.seed_bytes();
      const seedDigest = Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', new Uint8Array(seed).buffer)), (byte) => byte.toString(16).padStart(2, '0')).join('');
      await storage.put(seedKey, seed);
      await storage.put(proofKey, result.proof_bytes());
      const infoHash = Array.from(result.info_hash(), (byte) => byte.toString(16).padStart(2, '0')).join('');
      const finished = await db.prepare(`UPDATE seed_task SET status = 'ready', infoHash = ?, processedBytes = ?, seedDigest = ?, seedSize = ?,
        leaseToken = NULL, leaseUntil = NULL, updatedAt = ? WHERE fileId = ? AND leaseToken = ? AND status = 'running'
        AND leaseUntil > ? AND EXISTS (SELECT 1 FROM file_record f WHERE f.id = seed_task.fileId AND f.status = 'uploaded')`)
        .bind(infoHash, received, seedDigest, seed.byteLength, Date.now(), task.fileId, token, Date.now()).run();
      if (!finished.meta.changes) throw new Error('processingLeaseLost');
    } finally { result.free(); }
    return 'done';
  } catch (cause) {
    // A commit response can be lost after D1 has already marked the artifacts ready.
    // If the outcome cannot be read, retain objects rather than delete a possibly committed seed.
    const state = await db.prepare('SELECT status, seedKey FROM seed_task WHERE fileId = ?').bind(task.fileId)
      .first<{ status: string; seedKey: string | null }>();
    if (state?.status === 'ready' && state.seedKey === seedKey) return 'done';
    if (storage) for (const key of [seedKey, proofKey]) if (key) await storage.delete(key).catch(() => {});
    const code = cause instanceof Error && ['processingSourceChanged', 'processingLeaseLost'].includes(cause.message) ? cause.message : 'processingFailed';
    await db.prepare(`UPDATE seed_task SET status = 'failed', leaseToken = NULL, leaseUntil = NULL, errorCode = ?, updatedAt = ?
      WHERE fileId = ? AND leaseToken = ? AND status = 'running'`).bind(code, Date.now(), task.fileId, token).run();
    throw new Error(code);
  } finally {
    await reader?.cancel().catch(() => {});
    reader?.releaseLock();
    if (computer && !consumed) computer.free();
  }
}
