import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { build } from 'esbuild';
import { Miniflare, Log, LogLevel } from 'miniflare';
import { checks } from './contract-suite.ts';

test('the shared protocol vectors also pass in workerd', async () => {
  const bundle = await build({
    entryPoints: [fileURLToPath(new URL('./runtime-worker.ts', import.meta.url))],
    bundle: true, write: false, format: 'esm', platform: 'browser', target: 'es2022'
  });
  const runtime = new Miniflare({
    workers: [{ config: {
      name: 'track-contract', compatibilityDate: '2026-10-26',
      manifest: { mainModule: 'index.js', modules: { 'index.js': { type: 'esm', contents: bundle.outputFiles[0]!.text } } }
    } }],
    host: '127.0.0.1', port: 0, log: new Log(LogLevel.ERROR), telemetry: { enabled: false }
  });
  try {
    const response = await runtime.dispatchFetch('https://track-contract.invalid/');
    assert.equal(response.status, 200);
    assert.deepEqual(await response.json(), { passed: checks.length, errors: [] });
  } finally { await runtime.dispose(); }
});
