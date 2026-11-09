import { chromium, webkit } from 'playwright-core';
import { initSync, SeedBuilder } from '@q2q/wasm';
import { createRequire } from 'node:module';
import { createServer } from 'node:http';
import { mkdtemp, mkdir, writeFile, readFile, symlink, rm } from 'node:fs/promises';
import { dirname, join, resolve, sep } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { tmpdir } from 'node:os';
import assert from 'node:assert/strict';

export async function runBundleStorage(options = {}) {
// Reuse the repository's build tool, without making the SDK depend on an app.
// VITE_RESOLVE_FROM can select a workspace's Vite when multiple versions coexist.
const require = createRequire(process.env.VITE_RESOLVE_FROM
  ? pathToFileURL(resolve(fileURLToPath(new URL('../../../', import.meta.url)), process.env.VITE_RESOLVE_FROM)) : import.meta.url);
// Resolve the selected package's ESM entry, not require.resolve('vite')'s CJS shim.
const vitePackage = require.resolve('vite/package.json');
const viteManifest = JSON.parse(await readFile(vitePackage, 'utf8'));
const exported = viteManifest.exports['.'];
const viteEntry = typeof exported === 'string' ? exported : exported.import.default;
const { build, version: viteVersion } = await import(pathToFileURL(resolve(dirname(vitePackage), viteEntry)).href);
initSync({ module: await readFile(new URL('../../../crates/q2q-wasm/pkg/q2q_wasm_bg.wasm', import.meta.url)) });
const builder = new SeedBuilder(3); builder.update(new Uint8Array([97, 98, 99]));
const prepared = builder.finish('bundle.bin');
const seed = Array.from(prepared.seed_bytes()); const proof = Array.from(prepared.proof_bytes()); prepared.free();
const directory = await mkdtemp(join(tmpdir(), 'q2q-storage-bundle-'));
let context; let server;
try {
  await mkdir(join(directory, 'node_modules/@q2q'), { recursive: true });
  await symlink(fileURLToPath(new URL('..', import.meta.url)), join(directory, 'node_modules/@q2q/client-web'), 'junction');
  await writeFile(join(directory, 'index.html'), '<!doctype html><title>SDK bundle test</title><script type="module" src="/entry.js"></script>');
  await writeFile(join(directory, 'entry.js'), `
    import {opfsCache,browserState,createBlockVerifier,resumeCheckpoint,fileDestination} from '@q2q/client-web';
    (async()=>{
      const verifier=await createBlockVerifier({seed:new Uint8Array(${JSON.stringify(seed)}),proof:new Uint8Array(${JSON.stringify(proof)})});
      const cache=await opfsCache(); const entry=await cache.create({name:'bundle.bin',size:3n});
      const state=await browserState({namespace:'bundle-test'});
      const checkpoint=await state.create({content:verifier.content,targetId:entry.targetId});
      const session=await resumeCheckpoint({state,checkpointId:checkpoint.id,verifier,destination:entry.destination});
      await session.writeBlock(0n,new Uint8Array([97,98,99])); await session.finish();
      const root=await navigator.storage.getDirectory();
      const target=await root.getFileHandle('export.bin',{create:true});
      const receipt=await entry.exportTo(await fileDestination(target));
      const text=await(await target.getFile()).text();
      if(text!=='abc'||receipt.kind!=='staged')throw new Error('bundle storage mismatch');
      await state.discard(checkpoint.id);await state.close();verifier.close();
      await cache.remove(entry.id);await root.removeEntry('export.bin');
      window.bundleResult={ok:true};
    })().catch(error=>{window.bundleResult={ok:false,error:error.code??error.name};});
  `);
  await build({ root: directory, configFile: false, logLevel: 'error', build: { outDir: 'dist', emptyOutDir: true } });
  const dist = join(directory, 'dist');
  server = createServer(async (request, response) => {
    try {
      const pathname = new URL(request.url, 'http://localhost').pathname;
      const path = resolve(dist, '.' + (pathname === '/' ? '/index.html' : pathname));
      if (!path.startsWith(dist + sep)) { response.writeHead(404).end(); return; }
      response.setHeader('Content-Type', path.endsWith('.wasm') ? 'application/wasm'
        : path.endsWith('.js') ? 'text/javascript' : 'text/html');
      response.setHeader('Content-Security-Policy', "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'; connect-src 'self'");
      response.end(await readFile(path));
    } catch { response.writeHead(404).end(); }
  });
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
  const engine = options.engine ?? process.env.BROWSER ?? 'chromium';
  const type = { chromium, webkit }[engine];
  assert(type, 'bundle test supports chromium or webkit');
  context = await type.launchPersistentContext(join(directory, 'profile'), { headless: true,
    ...(engine === 'chromium' && process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}) });
  const page = await context.newPage();
  await page.goto(`http://127.0.0.1:${server.address().port}`);
  await page.waitForFunction(() => !!window.bundleResult, undefined, { timeout: 20000 });
  const result = await page.evaluate(() => window.bundleResult);
  assert.equal(result.ok, true, JSON.stringify(result));
  const report = { engine, browser: context.browser().version(), vite: viteVersion,
    packageExport: '@q2q/client-web', workerAndWasmUnderCsp: true, storageAndRecovery: 'passed' };
  console.log(JSON.stringify(report));
  return report;
} finally {
  await context?.close();
  if (server) await new Promise(resolve => server.close(resolve));
  await rm(directory, { recursive: true, force: true });
}

}
