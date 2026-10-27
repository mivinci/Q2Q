import { fileURLToPath } from 'node:url';
import { readFile } from 'node:fs/promises';
import { build } from 'esbuild';
import { Miniflare, Log, LogLevel } from 'miniflare';
import type { MiniflareOptions, MiniflareWorkerConfig } from 'miniflare';

export async function runtime(timeout = '5000', registry?: {punch_id:string; url:string; public_key:string}): Promise<Miniflare> {
  const bundle = await build({ entryPoints: [fileURLToPath(new URL('./state-worker.ts', import.meta.url))],
    bundle: true, write: false, format: 'esm', platform: 'browser', target: 'es2022', external: ['cloudflare:workers'] });
  function config(name: string, target = name): MiniflareWorkerConfig {
    return {
      name, compatibilityDate: '2026-10-26',
      manifest: { mainModule: 'index.js', modules: { 'index.js': { type: 'esm', contents: bundle.outputFiles[0]!.text } } },
      exports: name === target ? {
        RateLimiter: { type: 'durable-object', storage: 'sqlite' },
        TestPeer: { type: 'durable-object', storage: 'sqlite' }, FaultFile: { type: 'durable-object', storage: 'sqlite' },
        StoreHarness: { type: 'durable-object', storage: 'sqlite' }, TestDirectory: { type: 'durable-object', storage: 'sqlite' }
      } : {},
      env: {
        ...(registry ? {REGISTRY: {type: 'worker' as const, worker: 'sdk-registry', exportName: 'PunchDiscovery'}, REGISTRY_ENVIRONMENT: {type: 'text' as const, value: 'test'}} : {}),
        RATE_LIMIT: {type: 'durable-object', worker: target, exportName: 'RateLimiter'},
        TURN_URL: {type: 'text', value: 'turn:127.0.0.1:9'}, TURN_SECRET: {type: 'text', value: 'test-secret'},
        PEERS: { type: 'durable-object', worker: target, exportName: 'TestPeer' },
        FILES: { type: 'durable-object', worker: target, exportName: 'FaultFile' },
        HARNESS: { type: 'durable-object', worker: target, exportName: 'StoreHarness' },
        DISCOVERY: { type: 'durable-object', worker: target, exportName: 'TestDirectory' },
        DELIVERY_TIMEOUT_MS: { type: 'text', value: '1000' }, REQUEST_TIMEOUT_MS: { type: 'text', value: timeout }
      }
    };
  }
  const workers: MiniflareOptions['workers'] = [{config: config('track-state')}, {config: config('track-entry-two', 'track-state')}, {config: config('track-isolated')}];
  if (registry) {
    const rpc = await build({entryPoints: [fileURLToPath(new URL('../../registry/src/worker.ts', import.meta.url))], bundle: true,
      write: false, format: 'esm', platform: 'browser', target: 'es2022', external: ['cloudflare:workers']});
    workers.push({config: {name: 'sdk-registry', compatibilityDate: '2026-10-26', exports: {PunchDiscovery: {type: 'worker'}},
      manifest: {mainModule: 'index.js', modules: {'index.js': {type: 'esm', contents: rpc.outputFiles[0]!.text}}},
      env: {DB: {type: 'd1', id: 'sdk-registry'}, DISCOVERY_ENVIRONMENT: {type: 'text', value: 'test'}}}});
  }
  const options: MiniflareOptions = {
    workers,
    host: '127.0.0.1', port: 0, log: new Log(LogLevel.ERROR), telemetry: { enabled: false }, unsafeInspectDurableObjects: true
  };
  const mf = new Miniflare(options);
  if (registry) {
    try {
      const db = await mf.getD1Database('DB', 'sdk-registry');
      for (const migration of ['0001_nodes.sql', '0002_report_limits.sql']) {
        const sql = (await readFile(new URL(`../../registry/migrations/${migration}`, import.meta.url), 'utf8')).replace(/^\s*--.*$/gm, '');
        for (const statement of sql.split(';').map(value => value.trim()).filter(Boolean)) await db.prepare(statement).run();
      }
      const now = Math.floor(Date.now() / 1000);
      await db.prepare("INSERT INTO nodes VALUES(?, 'punch', 1, ?, ?, 'sdk-test', NULL, 1, NULL, ?, ?, ?)")
        .bind(registry.punch_id, registry.url, registry.public_key, now, now, now + 90).run();
    } catch (error) {await mf.dispose(); throw error;}
  }
  return mf;
}
