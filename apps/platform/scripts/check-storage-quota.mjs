// Real SQLite migrations and upload lifecycle; all provider operations stay in memory.
import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { registerHooks } from 'node:module';
import { DatabaseSync } from 'node:sqlite';

globalThis.storageQuotaEnv = { UPLOAD_LOCAL_PROXY: 'true' };
registerHooks({ resolve(specifier, context, next) {
  if (specifier === 'cloudflare:workers') return { url: 'data:text/javascript,export const env=globalThis.storageQuotaEnv', shortCircuit: true };
  if (specifier === '$app/env') return { url: 'data:text/javascript,export const dev=true', shortCircuit: true };
  if (specifier.startsWith('#lib/')) return next(new URL(`../src/lib/${specifier.slice(5).replace(/\.js$/, '.ts')}`, import.meta.url).href, context);
  try { return next(specifier, context); }
  catch (cause) {
    if (context.parentURL?.startsWith(new URL('../src/', import.meta.url).href) && specifier.startsWith('.') && specifier.endsWith('.js')) return next(specifier.slice(0, -3) + '.ts', context);
    throw cause;
  }
} });
const { getStorageUsage, reserveUpload } = await import('../src/lib/server/storage-quota.ts');
const { getPlanCatalog, getProjectEntitlements } = await import('../src/lib/server/plans.ts');
const { formatBytes } = await import('../src/lib/upload.ts');
const { createUpload, completeUpload, cancelUpload, requireUpload } = await import('../src/lib/server/uploads.ts');
const { deleteFile } = await import('../src/lib/server/files.ts');
const MiB = 1024 ** 2;
const rejected = (key) => (cause) => cause.status === 413 && cause.body.message === key;

function fixture() {
  const sql = new DatabaseSync(':memory:');
  sql.exec('PRAGMA foreign_keys = ON');
  const migrations = new URL('../migrations/', import.meta.url);
  for (const name of readdirSync(migrations).filter(n => n.endsWith('.sql')).sort()) sql.exec(readFileSync(new URL(name, migrations), 'utf8'));
  sql.exec(`INSERT INTO user(id,name,email,createdAt,updatedAt) VALUES ('owner','Owner','owner@test',0,0),('other','Other','other@test',0,0);
    INSERT INTO project VALUES ('project','Project',NULL,0),('other','Other',NULL,0);
    INSERT INTO project_member VALUES ('project','owner','owner',0),('other','other','owner',0);
    INSERT INTO storage_connection VALUES ('storage','project','platform-s3',0),('other-storage','other','platform-s3',0);`);
  let beforeRun;
  const db = { prepare(query) { return { async all() { return { results: sql.prepare(query).all() }; }, bind(...args) { return {
    async first() { const row = sql.prepare(query).get(...args); return row ? { ...row } : null; },
    async all() { return { results: sql.prepare(query).all(...args) }; },
    async run() { beforeRun?.(query); return { meta: { changes: Number(sql.prepare(query).run(...args).changes) } }; }
  }; } }; }, async batch(statements) {
    sql.exec('BEGIN');
    try { const results = []; for (const statement of statements) results.push(await statement.run()); sql.exec('COMMIT'); return results; }
    catch (cause) { sql.exec('ROLLBACK'); throw cause; }
  } };
  const objects = new Map();
  const state = { failAbort: false, failDelete: false, failInit: false, wrongSize: false };
  globalThis.storageQuotaEnv.DB = db;
  globalThis.storageQuotaEnv.STORAGE = {
    async createMultipartUpload(key) { if (state.failInit) throw new Error('fixture'); return { uploadId: key, async abort() {} }; },
    async head(key) { return objects.get(key) ?? null; },
    async delete(key) { if (state.failDelete) throw new Error('fixture'); objects.delete(key); },
    resumeMultipartUpload(key) { return {
      async abort() { if (state.failAbort) throw new Error('fixture'); },
      async complete() { const task = sql.prepare('SELECT * FROM upload_task WHERE objectKey = ?').get(key);
        const object = { size: task.size + (state.wrongSize ? 1 : 0), etag: 'fixture', version: 'fixture', customMetadata: { uploadTask: task.id } };
        objects.set(key, object); return object; }
    }; }
  };
  const plan = (id, status = 'active') => sql.prepare(`INSERT INTO project_subscription(projectId,planId,status,createdAt,updatedAt)
    VALUES ('project',?,?,0,0) ON CONFLICT(projectId) DO UPDATE SET planId=excluded.planId,status=excluded.status`).run(id, status);
  const file = (size, project = 'project', user = 'owner') => {
    const id = crypto.randomUUID();
    sql.prepare("INSERT INTO file_record VALUES (?,?,?,?,?,'pending',0,0)").run(id, project, user, 'fixture.bin', size); return id;
  };
  const reserve = (fileId, id = crypto.randomUUID(), connectionId = 'storage', now = Date.now()) => reserveUpload(db, {
    id, fileId, userId: 'owner', connectionId, objectKey: `fixture/${id}`, transport: 'local', bucketIdentity: 'local-r2', now
  });
  const start = (size) => createUpload('owner', file(size), { id: crypto.randomUUID() });
  const complete = async (id) => { const task = await requireUpload(id, 'owner'); await completeUpload(task,
    Array.from({ length: Math.ceil(task.size / task.partSize) }, (_, i) => ({ partNumber: i + 1, etag: 'fixture' }))); return task.fileId; };
  const used = async (size) => { const id = file(size); plan('creator'); await reserve(id);
    sql.prepare("UPDATE upload_task SET status='uploaded' WHERE fileId=?").run(id);
    sql.prepare("UPDATE file_record SET status='uploaded' WHERE id=?").run(id); plan('free'); return id; };
  return { sql, db, state, plan, file, reserve, start, complete, used, hook(fn) { beforeRun = fn; }, usage: () => getStorageUsage(db, 'project') };
}
async function check(name, body) {
  const f = fixture();
  try { f.catalog = await getPlanCatalog(f.db); await body(f); console.log(`PASS ${name}`); } finally { f.sql.close(); }
}

await check('catalog and subscription fallback enforce all three storage limits', async f => {
  assert.deepEqual(['free','creator','team'].map(id => f.catalog[id].storageBytes), [100 * MiB, 100 * 1024 ** 3, 1024 ** 4]);
  assert.equal(formatBytes(f.catalog.team.storageBytes, 'en'), '1 TiB');
  for (const id of ['free','creator','team']) { f.plan(id); assert.equal((await getProjectEntitlements(f.db, 'project')).limits.storageBytes, f.catalog[id].storageBytes); }
  f.plan('team', 'past_due'); assert.equal((await getProjectEntitlements(f.db, 'project')).limits.storageBytes, 100 * MiB);
});
await check('exact capacity succeeds and an extra byte is rejected', async f => {
  await f.used(99 * MiB); await f.start(MiB);
  await assert.rejects(f.start(1), rejected('uploadStorageLimit'));
  assert.deepEqual(await f.usage(), { usedBytes: 99 * MiB, reservedBytes: MiB });
});
for (const plan of ['creator', 'team']) await check(`${plan} enforces its full storage allowance`, async f => {
  f.plan(plan);
  let left = f.catalog[plan].storageBytes - MiB;
  while (left > 0) {
    const size = Math.min(left, f.catalog[plan].maxFileSizeBytes);
    const file = f.file(size); assert.equal((await f.reserve(file)).meta.changes, 1);
    f.sql.prepare("UPDATE upload_task SET status='uploaded' WHERE fileId=?").run(file);
    f.sql.prepare("UPDATE file_record SET status='uploaded' WHERE id=?").run(file);
    left -= size;
  }
  await f.start(MiB); await assert.rejects(f.start(1), rejected('uploadStorageLimit'));
  const usage = await f.usage(); assert.equal(usage.usedBytes + usage.reservedBytes, f.catalog[plan].storageBytes);
});
await check('competing uploads cannot reserve the same remaining space', async f => {
  await f.used(99 * MiB);
  const results = await Promise.allSettled([f.start(MiB), f.start(MiB)]);
  assert.equal(results.filter(r => r.status === 'fulfilled').length, 1);
  assert.equal(results.filter(r => r.status === 'rejected' && rejected('uploadStorageLimit')(r.reason)).length, 1);
  assert.equal((await f.usage()).reservedBytes, MiB);
});
await check('idempotent creation reserves once; completion moves reserved bytes to used', async f => {
  const task = await f.start(MiB); const row = await requireUpload(task.id, 'owner');
  await createUpload('owner', row.fileId, { id: task.id });
  assert.deepEqual(await f.usage(), { usedBytes: 0, reservedBytes: MiB });
  await f.complete(task.id); await completeUpload(await requireUpload(task.id, 'owner'), []);
  assert.deepEqual(await f.usage(), { usedBytes: MiB, reservedBytes: 0 });
});
await check('failed abort retains reservation until cleanup succeeds', async f => {
  const task = await f.start(MiB); f.state.failAbort = true;
  await cancelUpload(await requireUpload(task.id, 'owner'));
  assert.equal((await f.usage()).reservedBytes, MiB);
  f.state.failAbort = false; await cancelUpload(await requireUpload(task.id, 'owner'));
  assert.deepEqual(await f.usage(), { usedBytes: 0, reservedBytes: 0 });
});
await check('size mismatch keeps capacity reserved until failed object is removed', async f => {
  const task = await f.start(MiB); f.state.wrongSize = true;
  await assert.rejects(f.complete(task.id), e => e.status === 422);
  assert.equal((await f.usage()).reservedBytes, MiB);
  await cancelUpload(await requireUpload(task.id, 'owner'));
  assert.equal((await f.usage()).reservedBytes, 0);
});
await check('initialization failure releases unused reservation', async f => {
  f.state.failInit = true; const log = console.error; console.error = () => {};
  try { await assert.rejects(f.start(MiB), e => e.status === 502); } finally { console.error = log; }
  assert.deepEqual(await f.usage(), { usedBytes: 0, reservedBytes: 0 });
});
await check('failed deletion retains used space; successful retry releases it once', async f => {
  const task = await f.start(MiB); const id = await f.complete(task.id);
  f.state.failDelete = true; await assert.rejects(deleteFile(id, 'owner'));
  assert.equal((await f.usage()).usedBytes, MiB);
  f.state.failDelete = false; await deleteFile(id, 'owner'); await deleteFile(id, 'owner');
  assert.deepEqual(await f.usage(), { usedBytes: 0, reservedBytes: 0 });
});
await check('downgrade preserves existing reservations and content but blocks new uploads', async f => {
  f.plan('creator'); const task = await f.start(4 * MiB); const row = await requireUpload(task.id, 'owner');
  await f.used(100 * MiB);
  assert.equal((await createUpload('owner', row.fileId, { id: task.id })).id, task.id);
  await f.complete(task.id); await assert.rejects(f.start(1), rejected('uploadStorageLimit'));
  assert.equal((await f.usage()).usedBytes, 104 * MiB);
});
await check('subscription is rechecked atomically when reservation writes', async f => {
  await f.used(99 * MiB); f.plan('team');
  f.hook(query => { if (query.includes('INSERT INTO upload_task')) f.plan('free'); });
  await assert.rejects(f.start(2 * MiB), rejected('uploadStorageLimit'));
  assert.equal((await f.usage()).reservedBytes, 0);
});
await check('other project usage, guessed connections and revoked membership stay isolated', async f => {
  const own = f.file(MiB), other = f.file(MiB, 'other', 'other');
  assert.equal((await f.reserve(other)).meta.changes, 0);
  assert.equal((await f.reserve(own, crypto.randomUUID(), 'other-storage')).meta.changes, 0);
  f.sql.exec("DELETE FROM project_member WHERE projectId='project'");
  assert.equal((await f.reserve(own)).meta.changes, 0);
  assert.equal((await f.usage()).reservedBytes, 0);
});
await check('expired multipart reservations remain charged until explicit cancellation', async f => {
  const task = await f.start(MiB);
  f.sql.prepare('UPDATE upload_task SET expiresAt=0 WHERE id=?').run(task.id);
  assert.equal((await f.usage()).reservedBytes, MiB);
  await cancelUpload(await requireUpload(task.id, 'owner'));
  assert.equal((await f.usage()).reservedBytes, 0);
});

await check('editing the database catalog immediately changes reads and upload limits', async f => {
  f.sql.prepare("UPDATE plan_catalog SET maxFileSizeBytes = ?, storageBytes = ?, revision = revision + 1 WHERE id = 'free'").run(MiB, 2 * MiB);
  assert.equal((await getProjectEntitlements(f.db, 'project')).limits.storageBytes, 2 * MiB);
  await assert.rejects(f.start(2 * MiB), rejected('uploadPlanLimit'));
  assert.equal((await f.reserve(f.file(2 * MiB))).meta.changes, 0);
  await f.start(MiB); await f.start(MiB);
  await assert.rejects(f.start(1), rejected('uploadStorageLimit'));
  f.sql.prepare("UPDATE plan_catalog SET storageBytes = ?, revision = revision + 1 WHERE id = 'free'").run(3 * MiB);
  await f.start(MiB);
  assert.equal((await f.usage()).reservedBytes, 3 * MiB);
});
await check('catalog quota edits between preflight and reservation are checked atomically', async f => {
  await f.used(99 * MiB);
  f.hook(query => {
    if (query.includes('INSERT INTO upload_task')) f.sql.exec("UPDATE plan_catalog SET storageBytes = 104857600 - 1 WHERE id = 'free'");
  });
  await assert.rejects(f.start(MiB), rejected('uploadStorageLimit'));
  assert.equal((await f.usage()).reservedBytes, 0);
});
await check('removing storage capability at reservation prevents a preflight-approved upload', async f => {
  f.hook(query => {
    if (query.includes('INSERT INTO upload_task')) f.sql.exec("UPDATE plan_catalog SET platformStorage = 0 WHERE id = 'free'");
  });
  await assert.rejects(f.start(MiB), cause => cause.status === 403 && cause.body.message === 'planRequired');
  assert.equal((await f.usage()).reservedBytes, 0);
});
await check('subscription start and expiry boundaries agree with atomic file limits', async f => {
  const start = Date.now(); const end = start + 1000;
  for (const status of ['active','simulated']) {
    f.plan('creator', status);
    f.sql.prepare("UPDATE project_subscription SET periodStartAt = ?, periodEndAt = ? WHERE projectId = 'project'").run(start,end);
    for (const [now, expected] of [[start-1,'free'],[start,'creator'],[end-1,'creator'],[end,'free']]) {
      assert.equal((await getProjectEntitlements(f.db, 'project', now)).planId, expected);
      const inserted = await f.reserve(f.file(4 * MiB), crypto.randomUUID(), 'storage', now);
      assert.equal(inserted.meta.changes, expected === 'creator' ? 1 : 0);
    }
  }
});
await check('expiry changed during preflight prevents a paid-size reservation', async f => {
  f.plan('creator');
  f.hook(query => {
    if (query.includes('INSERT INTO upload_task')) f.sql.exec("UPDATE project_subscription SET periodEndAt = 0 WHERE projectId = 'project'");
  });
  await assert.rejects(f.start(4 * MiB), rejected('uploadPlanLimit'));
  assert.equal((await f.usage()).reservedBytes, 0);
});
await check('missing catalog produces a service error instead of code-default quotas', async f => {
  f.sql.exec("DELETE FROM plan_catalog WHERE id = 'free'");
  await assert.rejects(getPlanCatalog(f.db), cause => cause.status === 503);
  await assert.rejects(f.start(MiB), cause => cause.status === 503);
  assert.equal((await f.reserve(f.file(MiB))).meta.changes, 0);
});
await check('database rejects unsupported IDs, features and unsafe quota values', async f => {
  for (const query of [
    "UPDATE plan_catalog SET id = 'unknown' WHERE id = 'free'",
    "UPDATE plan_catalog SET storageBytes = -1 WHERE id = 'free'",
    "UPDATE plan_catalog SET maxFileSizeBytes = 8589934593 WHERE id = 'free'",
    "UPDATE plan_catalog SET platformStorage = 2 WHERE id = 'free'",
    "UPDATE plan_catalog SET maxFileSizeBytes = 1.5 WHERE id = 'free'"
  ]) assert.throws(() => f.sql.exec(query));
});
