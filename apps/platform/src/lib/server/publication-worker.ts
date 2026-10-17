import { createS3Storage, type S3Config } from './s3-storage.js';

interface Snapshot {
  id: string; fileId: string; status: string; size: number; transport: string; bucketIdentity: string;
  objectKey: string; objectVersion: string | null; objectEtag: string;
  preparedSeedKey: string; seedSize: number; seedDigest: string;
}
const hex = (bytes: ArrayBuffer) => Array.from(new Uint8Array(bytes), (byte) => byte.toString(16).padStart(2, '0')).join('');

export async function sweepPublications(db: D1Database, config: S3Config) {
  const now = Date.now();
  // Fence expired workers before selecting objects. Even a delayed worker cannot commit.
  await db.prepare(`UPDATE content_publication SET status = 'failed', errorCode = 'publicationFailed',
    leaseToken = NULL, leaseUntil = 0, updatedAt = ?
    WHERE status = 'creating' AND ((leaseUntil > 0 AND leaseUntil <= ?)
      OR (leaseUntil = 0 AND updatedAt < ?))`).bind(now, now, now - 600_000).run();
  await db.prepare(`UPDATE content_publication SET leaseToken = NULL, leaseUntil = 0
    WHERE status IN ('withdrawn', 'failed') AND leaseUntil > 0 AND leaseUntil <= ?`).bind(now).run();
  const identity = storageIdentity(config);
  const storage = createS3Storage(config);
  const { results } = await db.prepare(`SELECT a.token, a.publicationId, a.seedKey, a.proofKey
    FROM publication_artifact a JOIN content_publication p ON p.id = a.publicationId
    WHERE p.bucketIdentity = ? AND p.leaseUntil = 0
      AND (p.status <> 'published' OR a.seedKey IS NOT p.seedKey)
      AND (p.status <> 'withdrawn' OR p.lastSignedUntil <= ?)
    ORDER BY a.checkedAt, a.token LIMIT 25`).bind(identity, now)
    .all<{ token: string; publicationId: string; seedKey: string; proofKey: string }>();
  for (const artifact of results) {
    // The conditional retirement is another fence against a new creator acquiring a lease.
    const retired = await db.prepare(`UPDATE publication_artifact SET checkedAt = ?
      WHERE token = ? AND EXISTS (SELECT 1 FROM content_publication p WHERE p.id = publicationId
        AND p.leaseUntil = 0 AND p.status <> 'creating'
        AND (p.status <> 'published' OR p.seedKey IS NOT publication_artifact.seedKey)
        AND (p.status <> 'withdrawn' OR p.lastSignedUntil <= ?)) RETURNING token`)
      .bind(now, artifact.token, now).first();
    if (!retired) continue;
    try {
      await storage.delete(artifact.seedKey);
      await storage.delete(artifact.proofKey);
      await db.prepare('UPDATE publication_artifact SET retiredAt = COALESCE(retiredAt, ?) WHERE token = ?').bind(now, artifact.token).run();
      await refreshCleanup(db, artifact.publicationId);
    } catch {
      // Keep the reference on failure; other versions must still get their scan turn.
      continue;
    }
    // Retain the tombstone. A previously in-flight S3 PUT can arrive after DELETE;
    // subsequent scans remove it again instead of losing knowledge of its object keys.
  }
}

function storageIdentity(config: S3Config) {
  return JSON.stringify({ endpoint: new URL(config.endpoint).toString().replace(/\/$/, ''), region: config.region, bucket: config.bucket, forcePathStyle: config.forcePathStyle !== false });
}

async function refreshCleanup(db: D1Database, id: string) {
  await db.prepare(`UPDATE content_publication SET cleanupPending = EXISTS (
    SELECT 1 FROM publication_artifact a WHERE a.publicationId = content_publication.id
      AND a.retiredAt IS NULL AND (content_publication.status <> 'published' OR a.seedKey IS NOT content_publication.seedKey))
    WHERE id = ?`).bind(id).run();
}

export async function cleanupPublication(db: D1Database, config: S3Config, id: string) {
  const row = await db.prepare(`SELECT seedKey, proofKey, bucketIdentity FROM content_publication
    WHERE id = ? AND status = 'withdrawn' AND leaseUntil = 0 AND lastSignedUntil <= ?`)
    .bind(id, Date.now()).first<{ seedKey: string | null; proofKey: string | null; bucketIdentity: string }>();
  if (!row) return 'busy';
  const identity = JSON.stringify({ endpoint: new URL(config.endpoint).toString().replace(/\/$/, ''), region: config.region, bucket: config.bucket, forcePathStyle: config.forcePathStyle !== false });
  if (row.bucketIdentity !== identity) throw new Error('publicationSourceChanged');
  const storage = createS3Storage(config);
  if (row.seedKey) await storage.delete(row.seedKey);
  if (row.proofKey) await storage.delete(row.proofKey);
  await db.prepare('DELETE FROM publication_artifact WHERE publicationId = ? AND seedKey = ?').bind(id, row.seedKey).run();
  await db.prepare(`UPDATE content_publication SET cleanupPending = EXISTS (
    SELECT 1 FROM publication_artifact WHERE publicationId = ? AND retiredAt IS NULL) WHERE id = ?`).bind(id, id).run();
  return 'done';
}

export async function processPublication(db: D1Database, config: S3Config, id: string, proof: (seed: Uint8Array) => Uint8Array) {
  const token = crypto.randomUUID();
  const now = Date.now();
  const snapshot = await db.prepare(`UPDATE content_publication SET leaseToken = ?, leaseUntil = ?, updatedAt = ?
    WHERE id = ? AND status = 'creating' AND leaseUntil < ?
    AND EXISTS (SELECT 1 FROM file_record WHERE id = content_publication.fileId AND status = 'uploaded')
    RETURNING *`).bind(token, now + 120_000, now, id, now).first<Snapshot>();
  if (!snapshot) {
    const current = await db.prepare('SELECT status FROM content_publication WHERE id = ?').bind(id).first<{ status: string }>();
    return current?.status === 'creating' ? 'busy' : 'done';
  }
  const storage = createS3Storage(config);
  const prefix = `projects/publications/${id}/${token}`;
  const seedKey = `${prefix}/seed.q2q`;
  const proofKey = `${prefix}/proof.bin`;
  // Persist the object intent before writing S3. A crash cannot hide orphaned artifacts.
  const intent = await db.batch([
    db.prepare(`INSERT INTO publication_artifact (token, publicationId, seedKey, proofKey, createdAt)
      SELECT ?, id, ?, ?, ? FROM content_publication
      WHERE id = ? AND status = 'creating' AND leaseToken = ? AND leaseUntil > ?`)
      .bind(token, seedKey, proofKey, now, id, token, Date.now()),
    db.prepare(`UPDATE content_publication SET cleanupPending = 1 WHERE id = ? AND status = 'creating' AND leaseToken = ?`).bind(id, token)
  ]);
  if (!intent[0].meta.changes) {
    await db.prepare(`UPDATE content_publication SET leaseUntil = 0 WHERE id = ? AND leaseToken = ?`).bind(id, token).run();
    return 'done';
  }
  try {
    const identity = JSON.stringify({ endpoint: new URL(config.endpoint).toString().replace(/\/$/, ''), region: config.region, bucket: config.bucket, forcePathStyle: config.forcePathStyle !== false });
    if (snapshot.transport !== 's3' || snapshot.bucketIdentity !== identity) throw new Error('publicationSourceChanged');
    const source = await storage.get(snapshot.objectKey, { etag: snapshot.objectEtag, version: snapshot.objectVersion });
    const sourceLength = Number(source.headers.get('content-length'));
    await source.body?.cancel();
    if (!source.ok || sourceLength !== snapshot.size) throw new Error('publicationSourceChanged');
    if (snapshot.seedSize > 2 * 1024 * 1024) throw new Error('publicationInvalid');
    const response = await storage.get(snapshot.preparedSeedKey);
    if (!response.ok || Number(response.headers.get('content-length')) !== snapshot.seedSize) {
      await response.body?.cancel();
      throw new Error('publicationInvalid');
    }
    const bytes = new Uint8Array(await response.arrayBuffer());
    if (bytes.length !== snapshot.seedSize || hex(await crypto.subtle.digest('SHA-256', bytes)) !== snapshot.seedDigest) throw new Error('publicationInvalid');
    const proofBytes = proof(bytes);
    await storage.put(seedKey, bytes);
    await storage.put(proofKey, proofBytes);
    // Withdrawal and lease replacement always win over an in-flight publication.
    await db.prepare(`UPDATE content_publication SET status = 'published', seedKey = ?, proofKey = ?,
      leaseUntil = 0, errorCode = NULL, updatedAt = ? WHERE id = ? AND status = 'creating'
      AND leaseToken = ? AND leaseUntil > ?`).bind(seedKey, proofKey, Date.now(), id, token, Date.now()).run();
  } catch {
    // Read the commit outcome before touching S3: a successful D1 write may lose its response.
    const current = await db.prepare('SELECT seedKey FROM content_publication WHERE id = ?').bind(id).first<{ seedKey: string | null }>();
    if (current?.seedKey !== seedKey) {
      await db.prepare(`UPDATE content_publication SET status = 'failed', errorCode = 'publicationFailed',
        leaseUntil = 0, updatedAt = ? WHERE id = ? AND status = 'creating' AND leaseToken = ?`)
        .bind(Date.now(), id, token).run();
    }
  }
  const current = await db.prepare('SELECT seedKey FROM content_publication WHERE id = ?').bind(id).first<{ seedKey: string | null }>();
  if (current?.seedKey !== seedKey) {
    await storage.delete(seedKey);
    await storage.delete(proofKey);
    await db.prepare('DELETE FROM publication_artifact WHERE token = ?').bind(token).run();
  }
  await refreshCleanup(db, id);
  await db.prepare(`UPDATE content_publication SET leaseUntil = 0 WHERE id = ? AND leaseToken = ?`).bind(id, token).run();
  const withdrawn = await db.prepare("SELECT id FROM content_publication WHERE id = ? AND status = 'withdrawn'").bind(id).first();
  if (withdrawn) await cleanupPublication(db, config, id);
  return 'done';
}
