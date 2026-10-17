// Local SQLite + mock S3 + real Wasm. No credentials or external services are used.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { once } from 'node:events';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { registerHooks } from 'node:module';
import { DatabaseSync } from 'node:sqlite';
import { initSync, SeedBuilder, publication_proof } from '@q2q/wasm';

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
const { processPublication, cleanupPublication, sweepPublications } = await import('../src/lib/server/publication-worker.ts');
const grants = await import('../src/lib/server/download-grant-store.ts');
initSync({ module: readFileSync(new URL('../../../crates/q2q-wasm/pkg/q2q_wasm_bg.wasm', import.meta.url)) });
const bytes = Buffer.alloc(256 * 1024 + 17, 42);
const objects = new Map();
let reads = 0;
let rejectSource = false;
let rejectPut = false;
let rejectDelete = false;
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
    if (rejectDelete) { response.writeHead(503).end(); return; }
    objects.delete(key);
    response.writeHead(204).end();
  } else if (request.method === 'GET' && objects.has(key)) {
    response.writeHead(200, { 'Content-Length': objects.get(key).length }).end(objects.get(key));
  } else response.writeHead(404).end();
});
server.listen(0, '127.0.0.1');
await once(server, 'listening');
const config = { endpoint: `http://127.0.0.1:${server.address().port}`, region: 'test', bucket: 'test-bucket', accessKeyId: 'fixture', secretAccessKey: 'fixture', forcePathStyle: true };
const identity = JSON.stringify({ endpoint: config.endpoint, region: config.region, bucket: config.bucket, forcePathStyle: true });

function fixture(onCommit, onPublicationCommit) {
  const sql = new DatabaseSync(':memory:');
  sql.exec(`CREATE TABLE file_record(id TEXT PRIMARY KEY, status TEXT, name TEXT, size INTEGER, projectId TEXT, createdBy TEXT);
    CREATE TABLE project(id TEXT PRIMARY KEY); CREATE TABLE user(id TEXT PRIMARY KEY);
    INSERT INTO project VALUES ('project'); INSERT INTO user VALUES ('user');
    INSERT INTO user VALUES ('owner'), ('member'), ('outsider');
    CREATE TABLE project_member(projectId TEXT, userId TEXT, role TEXT, PRIMARY KEY(projectId, userId));
    INSERT INTO project_member VALUES ('project', 'user', 'member'), ('project', 'owner', 'owner'), ('project', 'member', 'member'), ('other-project', 'outsider', 'owner');
    CREATE TABLE upload_task(id TEXT PRIMARY KEY, fileId TEXT, status TEXT, objectKey TEXT, objectEtag TEXT, objectVersion TEXT, transport TEXT, bucketIdentity TEXT);`);
  sql.exec(readFileSync(new URL('../migrations/0008_seed_processing.sql', import.meta.url), 'utf8'));
  sql.exec(readFileSync(new URL('../migrations/0009_seed_integrity.sql', import.meta.url), 'utf8'));
  sql.exec(readFileSync(new URL('../migrations/0010_publications.sql', import.meta.url), 'utf8'));
  sql.exec(readFileSync(new URL('../migrations/0011_publication_jobs.sql', import.meta.url), 'utf8'));
  sql.exec(readFileSync(new URL('../migrations/0012_publication_recovery.sql', import.meta.url), 'utf8'));
  sql.exec(readFileSync(new URL('../migrations/0013_download_grants.sql', import.meta.url), 'utf8'));
  sql.prepare('INSERT INTO file_record VALUES (?, ?, ?, ?, ?, ?)').run('file', 'uploaded', 'fixture.bin', bytes.length, 'project', 'user');
  sql.prepare('INSERT INTO upload_task VALUES (?, ?, ?, ?, ?, ?, ?, ?)').run('upload', 'file', 'uploaded', 'source', 'snapshot', null, 's3', identity);
  sql.prepare('INSERT INTO seed_task (fileId, uploadId, status, createdAt, updatedAt) VALUES (?, ?, ?, ?, ?)').run('file', 'upload', 'pending', 0, 0);
  const db = { prepare(query) { return { bind(...parameters) { return {
    async first() { db.beforeFirst?.(query, sql); return sql.prepare(query).get(...parameters) ?? null; },
    async all() { return { results: sql.prepare(query).all(...parameters) }; },
    async run() {
      if (query.includes("SET status = 'ready'")) onCommit?.('before', sql);
      if (query.includes("SET status = 'published'")) onPublicationCommit?.('before', sql);
      const result = sql.prepare(query).run(...parameters);
      if (query.includes("SET status = 'ready'")) onCommit?.('after', sql);
      if (query.includes("SET status = 'published'")) onPublicationCommit?.('after', sql);
      return { meta: { changes: Number(result.changes) } };
    }
  }; } }; }, async batch(statements) {
    sql.exec('BEGIN');
    try { const result = []; for (const statement of statements) result.push(await statement.run()); sql.exec('COMMIT'); return result; }
    catch (cause) { sql.exec('ROLLBACK'); throw cause; }
  } };
  return { db, sql, row: () => sql.prepare('SELECT * FROM seed_task').get() };
}
const create = (size) => new SeedBuilder(size);
async function publicationFixture(onCommit) {
  const f = fixture(undefined, onCommit);
  await processSeed(f.db, config, { fileId: 'file' }, create);
  const seed = f.row();
  f.sql.prepare(`INSERT INTO content_publication (id, projectId, fileId, uploadId, createdBy, status,
    name, size, infoHash, objectKey, objectEtag, transport, bucketIdentity, preparedSeedKey,
    preparedProofKey, seedDigest, seedSize, createdAt, updatedAt)
    VALUES ('publication', 'project', 'file', 'upload', 'user', 'creating', 'fixture.bin', ?, ?,
      'source', 'snapshot', 's3', ?, ?, ?, ?, ?, ?, ?)`)
    .run(bytes.length, seed.infoHash, identity, seed.seedKey, seed.proofKey, seed.seedDigest, seed.seedSize, Date.now(), Date.now());
  return { ...f, publication: () => f.sql.prepare('SELECT * FROM content_publication').get() };
}
function withdraw(f) {
  f.sql.prepare("UPDATE content_publication SET status = 'withdrawn', withdrawnAt = ?, cleanupPending = 1").run(Date.now());
}
function abandoned(f, token = 'abandoned') {
  const seedKey = `orphan/${token}/seed`, proofKey = `orphan/${token}/proof`;
  f.sql.prepare(`INSERT INTO publication_artifact (token, publicationId, seedKey, proofKey, createdAt)
    VALUES (?, 'publication', ?, ?, 0)`).run(token, seedKey, proofKey);
  f.sql.prepare('UPDATE content_publication SET cleanupPending = 1, leaseToken = ?, leaseUntil = ?').run(token, Date.now() - 1);
  objects.set(seedKey, Buffer.from('orphan')); objects.set(proofKey, Buffer.from('orphan'));
  return { seedKey, proofKey };
}
async function check(name, body) {
  objects.clear(); reads = 0; rejectSource = false; rejectPut = false; rejectDelete = false;
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
  await check('publication copies verified artifacts and duplicate delivery is harmless', async () => {
    const f = await publicationFixture();
    try {
      const prepared = f.row();
      await processPublication(f.db, config, 'publication', publication_proof);
      const published = f.publication();
      assert.equal(published.status, 'published');
      assert.notEqual(published.seedKey, prepared.seedKey);
      assert.deepEqual(objects.get(published.seedKey), objects.get(prepared.seedKey));
      assert.deepEqual(objects.get(published.proofKey), objects.get(prepared.proofKey));
      await processPublication(f.db, config, 'publication', publication_proof);
      await sweepPublications(f.db, config);
      assert.ok(objects.has(published.seedKey));
      assert.equal(f.publication().cleanupPending, 0);
    } finally { f.sql.close(); }
  });
  await check('publication lost commit response preserves committed objects', async () => {
    const f = await publicationFixture((phase) => { if (phase === 'after') throw new Error('lost response'); });
    try {
      await processPublication(f.db, config, 'publication', publication_proof);
      assert.equal(f.publication().status, 'published');
      assert.ok(objects.has(f.publication().seedKey));
    } finally { f.sql.close(); }
  });
  await check('withdrawal during publication prevents commit and removes attempt objects', async () => {
    const f = await publicationFixture();
    try {
      await processPublication(f.db, config, 'publication', (seed) => { withdraw(f); return publication_proof(seed); });
      assert.equal(f.publication().status, 'withdrawn');
      assert.equal(f.publication().seedKey, null);
      assert.equal(f.publication().cleanupPending, 0);
      assert.equal(objects.size, 2); // Processing inputs remain intact.
    } finally { f.sql.close(); }
  });
  await check('tampered seed and changed source fail without publishing', async () => {
    for (const tamper of ['seed', 'source']) {
      const f = await publicationFixture();
      try {
        if (tamper === 'seed') objects.get(f.row().seedKey)[0] ^= 1;
        else rejectSource = true;
        await processPublication(f.db, config, 'publication', publication_proof);
        assert.equal(f.publication().status, 'failed');
        assert.equal(f.publication().cleanupPending, 0);
      } finally { rejectSource = false; f.sql.close(); }
    }
  });
  await check('expired attempt is fenced; failed deletion retries and late PUT is swept again', async () => {
    const f = await publicationFixture();
    try {
      const keys = abandoned(f);
      rejectDelete = true;
      await sweepPublications(f.db, config);
      assert.equal(f.publication().status, 'failed');
      assert.equal(f.publication().leaseToken, null);
      assert.equal(f.publication().cleanupPending, 1);
      assert.ok(objects.has(keys.seedKey));
      rejectDelete = false;
      await sweepPublications(f.db, config);
      assert.equal(f.publication().cleanupPending, 0);
      assert.equal(objects.has(keys.seedKey), false);
      objects.set(keys.seedKey, Buffer.from('late PUT'));
      await sweepPublications(f.db, config);
      assert.equal(objects.has(keys.seedKey), false);
      assert.equal(f.sql.prepare('SELECT COUNT(*) AS n FROM publication_artifact').get().n, 1);
      assert.equal(f.sql.prepare(`UPDATE content_publication SET status = 'published', seedKey = ?, proofKey = ?
        WHERE status = 'creating' AND leaseToken = 'abandoned' RETURNING id`).get(keys.seedKey, keys.proofKey), undefined);
    } finally { f.sql.close(); }
  });
  await check('active leases and outstanding signed links protect artifacts', async () => {
    const f = await publicationFixture();
    try {
      const keys = abandoned(f);
      f.sql.prepare('UPDATE content_publication SET leaseUntil = ?').run(Date.now() + 60_000);
      await sweepPublications(f.db, config);
      assert.equal(f.publication().status, 'creating');
      assert.ok(objects.has(keys.seedKey));
      withdraw(f);
      f.sql.prepare('UPDATE content_publication SET leaseUntil = 0, lastSignedUntil = ?').run(Date.now() + 60_000);
      await sweepPublications(f.db, config);
      assert.ok(objects.has(keys.seedKey));
      f.sql.exec('UPDATE content_publication SET lastSignedUntil = 0');
      await sweepPublications(f.db, config);
      assert.equal(objects.has(keys.seedKey), false);
    } finally { f.sql.close(); }
  });
  await check('withdrawn published version cleans up and cannot be revived', async () => {
    const f = await publicationFixture();
    try {
      await processPublication(f.db, config, 'publication', publication_proof);
      const key = f.publication().seedKey;
      withdraw(f);
      await cleanupPublication(f.db, config, 'publication');
      assert.equal(objects.has(key), false);
      assert.equal(f.publication().cleanupPending, 0);
      assert.throws(() => f.sql.exec("UPDATE content_publication SET status = 'creating'"), /invalid publication transition/);
    } finally { f.sql.close(); }
  });
  await check('replacement publication lease rejects the stale worker result', async () => {
    const f = await publicationFixture();
    try {
      await processPublication(f.db, config, 'publication', (seed) => {
        f.sql.exec("UPDATE content_publication SET leaseToken = 'replacement'");
        return publication_proof(seed);
      });
      assert.equal(f.publication().status, 'creating');
      assert.equal(f.publication().leaseToken, 'replacement');
      assert.equal(f.publication().seedKey, null);
      assert.equal(objects.size, 2);
      assert.equal(await processPublication(f.db, config, 'publication', publication_proof), 'busy');
    } finally { f.sql.close(); }
  });
  await check('bounded scan rotates beyond 25 tombstones and storage identity stays pinned', async () => {
    const f = await publicationFixture();
    try {
      const keys = Array.from({ length: 30 }, (_, index) => abandoned(f, `attempt-${index}`));
      await sweepPublications(f.db, { ...config, bucket: 'different-bucket' });
      assert.ok(keys.every((key) => objects.has(key.seedKey)));
      await sweepPublications(f.db, config);
      assert.equal(keys.filter((key) => objects.has(key.seedKey)).length, 5);
      assert.equal(f.publication().cleanupPending, 1);
      await sweepPublications(f.db, config);
      assert.ok(keys.every((key) => !objects.has(key.seedKey)));
      assert.equal(f.publication().cleanupPending, 0);
      assert.equal(f.sql.prepare('SELECT COUNT(*) AS n FROM publication_artifact').get().n, 30);
    } finally { f.sql.close(); }
  });
  await check('grant manager permissions, digest-only storage and terminal revocation', async () => {
    const f = await publicationFixture();
    try {
      await processPublication(f.db, config, 'publication', publication_proof);
      for (const user of ['member', 'outsider']) {
        await assert.rejects(grants.createDownloadGrant(f.db, 'publication', user, 'http://localhost'), { status: 404 });
        await assert.rejects(grants.listDownloadGrants(f.db, 'publication', user), { status: 404 });
      }
      const grant = await grants.createDownloadGrant(f.db, 'publication', 'user', 'http://localhost');
      const secret = new URL(grant.sourceUrl).searchParams.get('secret');
      const row = f.sql.prepare('SELECT * FROM download_grant').get();
      assert.equal(row.secretDigest, createHash('sha256').update(secret).digest('hex'));
      const list = await grants.listDownloadGrants(f.db, 'publication', 'owner');
      assert.equal(list.length, 1);
      assert.equal(JSON.stringify(list).includes(secret), false);
      assert.equal('secretDigest' in list[0], false);
      for (const user of ['member', 'outsider']) await assert.rejects(grants.revokeDownloadGrant(f.db, 'publication', grant.id, user), { status: 404 });
      await assert.rejects(grants.revokeDownloadGrant(f.db, 'different-version', grant.id, 'owner'), { status: 404 });
      await grants.revokeDownloadGrant(f.db, 'publication', grant.id, 'owner');
      const revokedAt = f.sql.prepare('SELECT revokedAt FROM download_grant').get().revokedAt;
      await grants.revokeDownloadGrant(f.db, 'publication', grant.id, 'user');
      assert.equal(f.sql.prepare('SELECT revokedAt FROM download_grant').get().revokedAt, revokedAt);
      await assert.rejects(grants.downloadLocation(f.db, config, 'publication', grant.id, secret, null), { status: 404 });
      assert.throws(() => f.sql.exec('UPDATE download_grant SET revokedAt = NULL'), /terminal/);
    } finally { f.sql.close(); }
  });
  await check('membership and publication are rechecked at grant mutation', async () => {
    for (const race of ['membership', 'withdrawal', 'revoke-membership']) {
      const f = await publicationFixture();
      try {
        await processPublication(f.db, config, 'publication', publication_proof);
        const grant = await grants.createDownloadGrant(f.db, 'publication', 'user', 'http://localhost');
        f.db.beforeFirst = (query, sql) => {
          if (query.includes(race === 'revoke-membership' ? 'UPDATE download_grant' : 'INSERT INTO download_grant')) {
            if (race === 'withdrawal') withdraw(f);
            else sql.exec("DELETE FROM project_member WHERE userId = 'user'");
          }
        };
        if (race === 'revoke-membership') {
          await assert.rejects(grants.revokeDownloadGrant(f.db, 'publication', grant.id, 'user'), { status: 404 });
          assert.equal(f.sql.prepare('SELECT revokedAt FROM download_grant').get().revokedAt, null);
        } else await assert.rejects(grants.createDownloadGrant(f.db, 'publication', 'user', 'http://localhost'), { status: 409 });
        assert.equal(f.sql.prepare('SELECT COUNT(*) AS n FROM download_grant').get().n, 1);
      } finally { f.sql.close(); }
    }
  });
  await check('signing reservation arbitrates withdrawal and retains deletion protection', async () => {
    for (const order of ['withdraw-first', 'reserve-first']) {
      const f = await publicationFixture();
      try {
        await processPublication(f.db, config, 'publication', publication_proof);
        const grant = await grants.createDownloadGrant(f.db, 'publication', 'user', 'http://localhost');
        const secret = new URL(grant.sourceUrl).searchParams.get('secret');
        const prepare = f.db.prepare;
        f.db.prepare = (query) => {
          const statement = prepare(query);
          if (!query.includes('SET lastSignedUntil')) return statement;
          return { bind(...parameters) {
            const bound = statement.bind(...parameters);
            return { async first() {
              if (order === 'withdraw-first') withdraw(f);
              const row = await bound.first();
              if (order === 'reserve-first') withdraw(f);
              return row;
            } };
          } };
        };
        if (order === 'withdraw-first') {
          await assert.rejects(grants.downloadLocation(f.db, config, 'publication', grant.id, secret, 'bytes=0-16'), { status: 404 });
          assert.equal(f.publication().lastSignedUntil, 0);
        } else {
          const url = new URL(await grants.downloadLocation(f.db, config, 'publication', grant.id, secret, 'bytes=0-16'));
          assert.equal(url.searchParams.get('X-Amz-Expires'), '240');
          assert.ok(f.publication().lastSignedUntil > Date.now() + 240_000);
          assert.equal(await cleanupPublication(f.db, config, 'publication'), 'busy');
          assert.ok(objects.has(f.publication().seedKey));
        }
      } finally { f.sql.close(); }
    }
  });
  await check('invalid ranges and wrong grant binding never yield a signed URL', async () => {
    const f = await publicationFixture();
    try {
      await processPublication(f.db, config, 'publication', publication_proof);
      const grant = await grants.createDownloadGrant(f.db, 'publication', 'owner', 'http://localhost');
      const secret = new URL(grant.sourceUrl).searchParams.get('secret');
      for (const range of ['bytes=-', 'bytes=0-1,4-5', 'bytes=20-1', 'bytes=-0', `bytes=${bytes.length}-`, 'bytes=9007199254740992-']) {
        await assert.rejects(grants.downloadLocation(f.db, config, 'publication', grant.id, secret, range), { status: 416 });
      }
      await assert.rejects(grants.downloadLocation(f.db, config, 'other-publication', grant.id, secret, null), { status: 404 });
      await assert.rejects(grants.downloadLocation(f.db, config, 'publication', grant.id, '0'.repeat(64), null), { status: 404 });
      for (const range of [null, 'bytes=0-16', 'bytes=16-', 'bytes=-17']) assert.ok(await grants.downloadLocation(f.db, config, 'publication', grant.id, secret, range));
    } finally { f.sql.close(); }
  });
  await check('slow signing cannot outlive its reserved deletion protection', async () => {
    const f = await publicationFixture();
    const realNow = Date.now;
    try {
      await processPublication(f.db, config, 'publication', publication_proof);
      const grant = await grants.createDownloadGrant(f.db, 'publication', 'user', 'http://localhost');
      const secret = new URL(grant.sourceUrl).searchParams.get('secret');
      f.db.beforeFirst = (query) => {
        if (query.includes('SET lastSignedUntil')) Date.now = () => realNow() + 56_000;
      };
      await assert.rejects(grants.downloadLocation(f.db, config, 'publication', grant.id, secret, null), (cause) => {
        assert.equal(cause.status, 503);
        assert.equal(JSON.stringify(cause).includes(secret), false);
        return true;
      });
    } finally { Date.now = realNow; f.sql.close(); }
  });
} finally { server.close(); await once(server, 'close'); }
