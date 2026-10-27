import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { build } from 'esbuild';
import { exportJWK, generateKeyPair } from 'jose';
import { Miniflare, Log, LogLevel } from 'miniflare';

test('named readonly RPC works without JWT; HTTP and writes remain protected', {timeout: 30000}, async () => {
  const bundle = await build({entryPoints: [fileURLToPath(new URL('../src/worker.ts', import.meta.url))], bundle: true,
    write: false, format: 'esm', platform: 'browser', target: 'es2022', external: ['cloudflare:workers']});
  const pair = await generateKeyPair('EdDSA', {crv: 'Ed25519'});
  const jwks = JSON.stringify({keys: [{...await exportJWK(pair.publicKey), kid: 'test'}]});
  const mf = new Miniflare({workers: [{config: {
    name: 'rpc-caller', compatibilityDate: '2026-10-26',
    manifest: {mainModule: 'index.js', modules: {'index.js': {type: 'esm', contents: `export default {async fetch(request, env) {
      const page = await env.REGISTRY.getPunches({limit: 1});
      let writeRejected = false, invalidRejected = false;
      try {await env.REGISTRY.report();} catch {writeRejected = true;}
      try {await env.REGISTRY.getPunches({limit: 1000});} catch {invalidRejected = true;}
      return Response.json({page, writeRejected, invalidRejected});
    }};`}}},
    env: {REGISTRY: {type: 'worker', worker: 'rpc-registry', exportName: 'PunchDiscovery'}}
  }}, {config: {
    name: 'rpc-registry', compatibilityDate: '2026-10-26',
    manifest: {mainModule: 'index.js', modules: {'index.js': {type: 'esm', contents: bundle.outputFiles[0]!.text}}},
    exports: {PunchDiscovery: {type: 'worker'}},
    env: {DB: {type: 'd1', id: 'registry-rpc-test'}, REGISTRY_JWKS: {type: 'text', value: jwks}, DISCOVERY_ENVIRONMENT: {type: 'text', value: 'test'}}
  }}], host: '127.0.0.1', port: 0, log: new Log(LogLevel.ERROR), telemetry: {enabled: false}});
  try {
    const db = await mf.getD1Database('DB', 'rpc-registry');
    for (const migration of ['0001_nodes.sql', '0002_report_limits.sql']) {
      const source = (await readFile(new URL(`../migrations/${migration}`, import.meta.url), 'utf8')).replace(/^\s*--.*$/gm, '');
      for (const sql of source.split(';').map(value => value.trim()).filter(Boolean)) await db.prepare(sql).run();
    }
    const now = Math.floor(Date.now() / 1000);
    await db.prepare(`INSERT INTO nodes VALUES('punch', 'punch', 1, 'quic://punch.invalid:443', ?, 'test', NULL, 1, NULL, ?, ?, ?)`)
      .bind(btoa(String.fromCharCode(...new Uint8Array(32))), now, now, now + 90).run();
    const response = await mf.dispatchFetch('https://caller.invalid/');
    assert.equal(response.status, 200);
    const result = await response.json() as {page:{version:number; environment:string; nodes:Record<string,unknown>[]}; writeRejected:boolean; invalidRejected:boolean};
    assert.equal(result.writeRejected, true); assert.equal(result.invalidRejected, true);
    assert.equal(result.page.version, 1); assert.equal(result.page.environment, 'test');
    assert.equal(result.page.nodes.length, 1);
    assert.deepEqual(Object.keys(result.page.nodes[0]!).sort(), ['accepting_new_connections', 'expires_at', 'identity_version', 'last_seen', 'metrics', 'public_key', 'punch_id', 'region', 'url']);
    const registry = await mf.getWorker('rpc-registry');
    assert.equal((await registry.fetch('https://registry.invalid/v1/discovery/punches', {headers: {'X-Internal': 'true'}})).status, 401);
    assert.equal((await registry.fetch('https://registry.invalid/getPunches')).status, 404);
    assert.equal(await db.prepare('SELECT count(*) FROM nodes').first('count(*)'), 1);
  } finally {await mf.dispose();}
});
