// Local SQLite + mock S3 + real Wasm. No credentials or external services are used.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { once } from 'node:events';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { registerHooks } from 'node:module';
import { DatabaseSync } from 'node:sqlite';
import { initSync, SeedBuilder } from '@q2q/wasm';

// Runtime TypeScript preserves .js specifiers; resolve only this project's relative source imports.
registerHooks({ resolve(specifier, context, next) {
  try { return next(specifier, context); }
  catch (cause) {
    if (context.parentURL?.startsWith(new URL('../src/', import.meta.url).href) && specifier.startsWith('.') && specifier.endsWith('.js')) {
      return next(specifier.slice(0, -3) + '.ts', context);
    }
    throw cause;
  }
} });
const { processSeed } = await import('../src/lib/server/seed-worker.ts');
initSync({ module: readFileSync(new URL('../../../crates/q2q-wasm/pkg/q2q_wasm_bg.wasm', import.meta.url)) });
const bytes = Buffer.alloc(256 * 1024 + 17, 42);
const objects = new Map();
let reads = 0;
let rejectSource = false;
let rejectPut = false;
const server = createServer(async (request, response) => {
  const key = decodeURIComponent(new URL(request.url, 'http://localhost').pathname).replace('/test-bucket/', '');
  if (request.method === 'GET' && key === 'source') {
    reads++;
    if (rejectSource || request.headers['if-match'] !== '"snapshot"') { response.writeHead(412).end(); return; }
    response.writeHead(200, { ETag: '"snapshot"', 'Content-Length': bytes.length }).end(bytes);
  } else if (request.method === 'PUT') {
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    if (rejectPut) { response.writeHead(503).end(); return; }
    objects.set(key, Buffer.concat(chunks));
    response.writeHead(200).end();
  } else if (request.method === 'DELETE') {
    objects.delete(key);
    response.writeHead(204).end();
  } else response.writeHead(404).end();
});
server.listen(0, '127.0.0.1');
await once(server, 'listening');
const config = { endpoint: `http://127.0.0.1:${server.address().port}`, region: 'test', bucket: 'test-bucket', accessKeyId: 'fixture', secretAccessKey: 'fixture', forcePathStyle: true };
const identity = JSON.stringify({ endpoint: config.endpoint, region: config.region, bucket: config.bucket, forcePathStyle: true });

function fixture(onCommit) {
  const sql = new DatabaseSync(':memory:');
  sql.exec(`CREATE TABLE file_record(id TEXT PRIMARY KEY, status TEXT, name TEXT, size INTEGER);
    CREATE TABLE upload_task(id TEXT PRIMARY KEY, fileId TEXT, status TEXT, objectKey TEXT, objectEtag TEXT, objectVersion TEXT, transport TEXT, bucketIdentity TEXT);`);
  sql.exec(readFileSync(new URL('../migrations/0008_seed_processing.sql', import.meta.url), 'utf8'));
  sql.exec(readFileSync(new URL('../migrations/0009_seed_integrity.sql', import.meta.url), 'utf8'));
  sql.prepare('INSERT INTO file_record VALUES (?, ?, ?, ?)').run('file', 'uploaded', 'fixture.bin', bytes.length);
  sql.prepare('INSERT INTO upload_task VALUES (?, ?, ?, ?, ?, ?, ?, ?)').run('upload', 'file', 'uploaded', 'source', 'snapshot', null, 's3', identity);
  sql.prepare('INSERT INTO seed_task (fileId, uploadId, status, createdAt, updatedAt) VALUES (?, ?, ?, ?, ?)').run('file', 'upload', 'pending', 0, 0);
  const db = { prepare(query) { return { bind(...parameters) { return {
    async first() { return sql.prepare(query).get(...parameters) ?? null; },
    async run() {
      if (query.includes("SET status = 'ready'")) onCommit?.('before', sql);
      const result = sql.prepare(query).run(...parameters);
      if (query.includes("SET status = 'ready'")) onCommit?.('after', sql);
      return { meta: { changes: Number(result.changes) } };
    }
  }; } }; } };
  return { db, sql, row: () => sql.prepare('SELECT * FROM seed_task').get() };
}
const create = (size) => new SeedBuilder(size);
async function check(name, body) {
  objects.clear(); reads = 0; rejectSource = false; rejectPut = false;
  await body();
  console.log(`PASS ${name}`);
}
try {
  await check('streaming output, inline proof and ready-message idempotency', async () => {
    const f = fixture();
    try {
      assert.equal(await processSeed(f.db, config, { fileId: 'file' }, create), 'done');
      const row = f.row();
      assert.equal(row.status, 'ready');
      assert.equal(row.processedBytes, bytes.length);
      const expected = new SeedBuilder(bytes.length);
      expected.update(bytes);
      const result = expected.finish_inline('fixture.bin');
      assert.deepEqual(objects.get(row.seedKey), Buffer.from(result.seed_bytes()));
      assert.equal(row.seedDigest, createHash('sha256').update(objects.get(row.seedKey)).digest('hex'));
      assert.equal(row.seedSize, objects.get(row.seedKey).length);
      assert.deepEqual(objects.get(row.proofKey), Buffer.from(result.proof_bytes()));
      result.free();
      await processSeed(f.db, config, { fileId: 'file' }, create);
      assert.equal(reads, 1);
      assert.equal(f.row().attempt, 1);
    } finally { f.sql.close(); }
  });
  await check('source change fails durably and retry succeeds', async () => {
    const f = fixture();
    try {
      rejectSource = true;
      await assert.rejects(processSeed(f.db, config, { fileId: 'file' }, create), /processingSourceChanged/);
      assert.equal(f.row().status, 'failed');
      rejectSource = false;
      await processSeed(f.db, config, { fileId: 'file' }, create);
      assert.equal(f.row().status, 'ready');
      assert.equal(f.row().attempt, 2);
    } finally { f.sql.close(); }
  });
  await check('storage failure remains retryable', async () => {
    const f = fixture();
    try {
      rejectPut = true;
      await assert.rejects(processSeed(f.db, config, { fileId: 'file' }, create), /processingFailed/);
      assert.equal(f.row().status, 'failed');
      assert.equal(objects.size, 0);
    } finally { f.sql.close(); }
  });
  await check('lost D1 commit response retains committed artifacts', async () => {
    const f = fixture((phase) => { if (phase === 'after') throw new Error('lost response'); });
    try {
      assert.equal(await processSeed(f.db, config, { fileId: 'file' }, create), 'done');
      assert.equal(f.row().status, 'ready');
      assert.equal(objects.size, 2);
    } finally { f.sql.close(); }
  });
  await check('replaced lease fences a stale result', async () => {
    const f = fixture((phase, sql) => { if (phase === 'before') sql.exec("UPDATE seed_task SET leaseToken = 'replacement'"); });
    try {
      await assert.rejects(processSeed(f.db, config, { fileId: 'file' }, create), /processingLeaseLost/);
      assert.equal(f.row().status, 'running');
      assert.equal(f.row().leaseToken, 'replacement');
      assert.equal(objects.size, 0);
    } finally { f.sql.close(); }
  });
  await check('active lease and deleted file never read S3', async () => {
    const f = fixture();
    try {
      f.sql.prepare("UPDATE seed_task SET status = 'running', leaseUntil = ?").run(Date.now() + 60_000);
      assert.equal(await processSeed(f.db, config, { fileId: 'file' }, create), 'busy');
      f.sql.exec("UPDATE seed_task SET status = 'pending'; UPDATE file_record SET status = 'deleted'");
      assert.equal(await processSeed(f.db, config, { fileId: 'file' }, create), 'done');
      assert.equal(reads, 0);
    } finally { f.sql.close(); }
  });
  await check('expired lease cannot commit and can be reclaimed', async () => {
    const f = fixture();
    try {
      const expired = (size) => {
        const inner = new SeedBuilder(size);
        return {
          update(chunk) { inner.update(chunk); f.sql.exec('UPDATE seed_task SET leaseUntil = 0'); },
          finish_inline(name) { return inner.finish_inline(name); },
          free() { inner.free(); }
        };
      };
      await assert.rejects(processSeed(f.db, config, { fileId: 'file' }, expired), /processingLeaseLost/);
      assert.equal(objects.size, 0);
      f.sql.exec("UPDATE seed_task SET status = 'running', leaseUntil = 0");
      await processSeed(f.db, config, { fileId: 'file' }, create);
      assert.equal(f.row().status, 'ready');
    } finally { f.sql.close(); }
  });
} finally { server.close(); await once(server, 'close'); }
