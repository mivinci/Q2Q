import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { build } from 'esbuild';
import { exportJWK, generateKeyPair } from 'jose';
import { Miniflare, Log, LogLevel } from 'miniflare';
import type { MiniflareWorkerConfig } from 'miniflare';
import { acceptsNewPeer } from '@q2q/punch-discovery';

async function setup(binding: 'correct' | 'wrong' | 'missing' = 'correct', environment = 'test') {
  const bundles = await Promise.all(['./state-worker.ts', './registry-worker.ts'].map(name => build({
    entryPoints: [fileURLToPath(new URL(name, import.meta.url))], bundle: true, write: false, format: 'esm', platform: 'browser', target: 'es2022', external: ['cloudflare:workers']
  })));
  const manifest = (index: number) => ({mainModule: 'index.js', modules: {'index.js': {type: 'esm' as const, contents: bundles[index]!.outputFiles[0]!.text}}});
  const key = await generateKeyPair('EdDSA', {crv: 'Ed25519'});
  const jwks = JSON.stringify({keys: [{...await exportJWK(key.publicKey), kid: 'test'}]});
  const track: MiniflareWorkerConfig = {
    name: 'discovery-track', compatibilityDate: '2026-10-26', manifest: manifest(0),
    exports: {TestDirectory: {type: 'durable-object', storage: 'sqlite'}},
    env: {DISCOVERY: {type: 'durable-object', exportName: 'TestDirectory', worker: 'discovery-track'},
      REGISTRY_ENVIRONMENT: {type: 'text', value: environment}, REQUEST_TIMEOUT_MS: {type: 'text', value: '500'},
      ...(binding === 'missing' ? {} : {REGISTRY: {type: 'worker' as const, worker: 'discovery-registry', ...(binding === 'correct' ? {exportName: 'PunchDiscovery'} : {})}})}
  };
  const mf = new Miniflare({workers: [{config: track}, {config: {
    name: 'discovery-registry', compatibilityDate: '2026-10-26', manifest: manifest(1), exports: {PunchDiscovery: {type: 'worker'}, Gate: {type: 'durable-object', storage: 'sqlite'}},
    env: {GATE: {type: 'durable-object', worker: 'discovery-registry', exportName: 'Gate'}, DB: {type: 'd1', id: 'discovery-test'}, DISCOVERY_ENVIRONMENT: {type: 'text', value: 'test'}, REGISTRY_JWKS: {type: 'text', value: jwks}}
  }}], host: '127.0.0.1', port: 0, log: new Log(LogLevel.ERROR), telemetry: {enabled: false}, unsafeInspectDurableObjects: true});
  const db = await mf.getD1Database('DB', 'discovery-registry').catch(async error => {await mf.dispose(); throw error;});
  try {
    for (const migration of ['0001_nodes.sql', '0002_report_limits.sql']) {
      const sql = (await readFile(new URL(`../../registry/migrations/${migration}`, import.meta.url), 'utf8')).replace(/^\s*--.*$/gm, '');
      for (const statement of sql.split(';').map(value => value.trim()).filter(Boolean)) await db.prepare(statement).run();
    }
  } catch (error) {await mf.dispose(); throw error;}
  const registry = await mf.getWorker('discovery-registry');
  const keyText = 'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=';
  async function seed(count: number) {
    const now = Math.floor(Date.now() / 1000);
    for (let start = 0; start < count; start += 50) await db.batch(Array.from({length: Math.min(50, count - start)}, (_, index) => db.prepare(`INSERT OR REPLACE INTO nodes
      (node_id, service_type, identity_version, advertise_url, public_key, version, region, accepting_new_connections, metrics_json, created_at, last_seen, expires_at)
      VALUES(?, 'punch', 1, 'quic://punch.invalid:443', ?, 'test', NULL, 1, NULL, ?, ?, ?)`)
      .bind(`p${String(start + index).padStart(4, '0')}`, keyText, now, now, now + 90)));
  }
  const snapshot = () => mf.dispatchFetch('https://track.invalid/discovery-test');
  const operation = (name: string) => mf.dispatchFetch(`https://track.invalid/discovery-test?operation=${name}`);
  const mode = (name: string) => registry.fetch(`https://registry.invalid/test-mode?mode=${name}`);
  return {mf, db, registry, seed, snapshot, operation, mode};
}
type Snapshot = {nodes:{punch_id:string; url:string; public_key:string; identity_version:number; accepting_new_connections:boolean}[]; expires_at:number};

test('real readonly Registry RPC needs no JWT, merges cold refreshes and bounds pagination', {timeout: 30000}, async () => {
  const fixture = await setup();
  const {mf, registry, seed, snapshot} = fixture;
  try {
    await seed(201);
    const responses = await Promise.all(Array.from({length: 12}, snapshot));
    for (const response of responses) {assert.equal(response.status, 200); assert.equal((await response.json() as Snapshot).nodes.length, 201);}
    assert.deepEqual(await (await registry.fetch('https://registry.invalid/test-stats')).json(), {calls: 2});
    for (const path of ['/v1/discovery/punches', '/v1/nodes', '/v1/nodes/self']) {
      const denied = await registry.fetch('https://registry.invalid' + path, {method: path.endsWith('/self') ? 'PUT' : 'GET', headers: {'X-Internal': 'true'}});
      assert.equal(denied.status, 401);
    }
    assert.equal((await fixture.operation('write')).status, 503);
    const status = await (await fixture.operation('status')).json() as {alarm:number};
    assert.ok(status.alarm > Date.now() && status.alarm <= Date.now() + 30000);
  } finally {await mf.dispose();}
});

test('failed/incomplete RPC refresh never publishes partial pages or extends old leases', {timeout: 30000}, async () => {
  const {mf, registry, seed, snapshot, operation, mode} = await setup();
  try {
    await seed(1);
    const previous = await (await snapshot()).json() as Snapshot;
    await seed(201);
    for (const failure of ['fail-second', 'bad-cursor', 'wrong-env', 'timeout']) {
      await mode(failure); await operation('refresh');
      const kept = await (await snapshot()).json() as Snapshot;
      assert.deepEqual(kept, previous);
      if (failure === 'timeout') {
        const before = await (await registry.fetch('https://registry.invalid/test-stats')).json();
        await operation('refresh'); // A timed-out but unsettled RPC still occupies its slot.
        assert.deepEqual(await (await registry.fetch('https://registry.invalid/test-stats')).json(), before);
      }
      await mode('ok');
    }
    await mode('fail'); await operation('expire');
    const unavailable = await snapshot(); assert.equal(unavailable.status, 503);
    assert.deepEqual(await unavailable.json(), {code: 'discovery_unavailable'});
    await mode('ok');
    let recovered = false;
    for (let attempt = 0; attempt < 50; attempt++) {
      await operation('refresh');
      const response = await snapshot();
      if (response.ok && (await response.json() as Snapshot).nodes.length === 201) {recovered = true; break;}
      await new Promise(resolve => setTimeout(resolve, 10));
    }
    assert.ok(recovered, 'discovery did not recover after the timed-out RPC settled');
  } finally {await mf.dispose();}
});

test('identity watermarks survive retirement and eviction; paused Punches remain discoverable', {timeout: 30000}, async () => {
  const {mf, db, seed, snapshot, operation} = await setup();
  try {
    await seed(1); await snapshot();
    await db.prepare("UPDATE nodes SET identity_version=2, advertise_url='quic://moved.invalid:443', accepting_new_connections=0 WHERE node_id='p0000'").run();
    await operation('refresh');
    const moved = await (await snapshot()).json() as Snapshot;
    assert.equal(moved.nodes[0]!.url, 'quic://moved.invalid:443');
    assert.equal(acceptsNewPeer(moved.nodes[0]!), false);
    const retiredAt = Math.floor(Date.now() / 1000);
    await db.prepare('UPDATE nodes SET created_at=?, last_seen=?, expires_at=?').bind(retiredAt - 200, retiredAt - 100, retiredAt - 10).run(); await operation('refresh');
    assert.equal((await (await snapshot()).json() as Snapshot).nodes.length, 0);
    await mf.unsafeEvictDurableObject('discovery-track', 'TestDirectory', {name: 'punch-discovery:v1'});
    const now = Math.floor(Date.now() / 1000);
    await db.prepare('UPDATE nodes SET identity_version=1, expires_at=?, last_seen=?').bind(now + 90, now).run();
    await operation('refresh');
    assert.equal((await (await snapshot()).json() as Snapshot).nodes.length, 0);
    await db.prepare('UPDATE nodes SET identity_version=3, public_key=?').bind('BAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=').run(); await operation('refresh');
    const rotated = await (await snapshot()).json() as Snapshot;
    assert.equal(rotated.nodes[0]!.identity_version, 3);
    assert.equal(rotated.nodes[0]!.public_key, 'BAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=');
    await db.prepare('UPDATE nodes SET public_key=?').bind('AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=').run(); await operation('refresh');
    assert.deepEqual(await (await snapshot()).json(), rotated);
  } finally {await mf.dispose();}
});

test('missing/wrong bindings and environment mismatches fail closed', {timeout: 30000}, async () => {
  for (const [binding, environment] of [['missing', 'test'], ['wrong', 'test'], ['correct', 'staging']] as const) {
    const {mf, seed, snapshot} = await setup(binding, environment);
    try {await seed(1); assert.equal((await snapshot()).status, 503);} finally {await mf.dispose();}
  }
});
