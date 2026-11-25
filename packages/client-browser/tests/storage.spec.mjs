import { cases as witnessCases } from './witness.browser.ts';
import { test as base, expect, chromium, firefox, webkit } from '@playwright/test';
import { mkdir, rm, readFile } from 'node:fs/promises';
import { httpFixture } from './http-fixture.mjs';
import { cases as httpCases } from './http.browser.ts';
import { startServer } from './browser-server.mjs';
import { cases as fileCases } from './file-destination.browser.ts';
import { cases as cacheCases } from './opfs-cache.browser.ts';
import { cases as checkpointCases } from './checkpoint.browser.ts';
import { runBundleStorage } from './bundle-storage.mjs';
import { runLargeStorage } from './large-storage.mjs';

const test = base.extend({
  origin: [async ({}, use) => {
    const { server, origin } = await startServer({ handleRequest: httpFixture() });
    try { await use(origin); }
    finally { await new Promise(resolve => server.close(resolve)); }
  }, { scope: 'worker' }],
  context: async ({ browserName }, use, testInfo) => {
    const profile = testInfo.outputPath('profile');
    await mkdir(profile, { recursive: true });
    const context = await { chromium, firefox, webkit }[browserName].launchPersistentContext(profile, {
      headless: process.env.HEADLESS !== '0', timeout: 30000,
      viewport: { width: 1280, height: 720 },
      ...(browserName === 'chromium' && process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {})
    });
    await context.tracing.start({ screenshots: true, snapshots: true, sources: true });
    try { await use(context); }
    finally {
      if (testInfo.status !== testInfo.expectedStatus) {
        await context.pages()[0]?.screenshot({ path: testInfo.outputPath('failure.png'), timeout: 5000 }).catch(() => {});
        await context.tracing.stop({ path: testInfo.outputPath('trace.zip') }).catch(() => {});
      } else { await context.tracing.stop(); }
      await context.close();
      await rm(profile, { recursive: true, force: true });
    }
  }
});

for (const [group, path, cases] of [
  ['file', '/tests/file-destination.browser.ts', fileCases],
  ['cache', '/tests/opfs-cache.browser.ts', cacheCases],
  ['checkpoint', '/tests/checkpoint.browser.ts', checkpointCases],
  ['http', '/tests/http.browser.ts', httpCases],
  ['witness', '/tests/witness.browser.ts', witnessCases]
]) {
  test.describe(`${group} ${group === 'http' ? '@http' : '@storage'}`, () => {
    for (const name of Object.keys(cases)) {
      test(name, async ({ page, origin }) => {
        await page.goto(origin);
        await page.evaluate(async ({ path, name }) => (await import(path)).cases[name](), { path, name });
      });
    }
  });
}

test('HTTP checkpoint resumes after actual page reload @http', async ({ page, origin }) => {
  await page.goto(origin);
  const saved = await page.evaluate(async () => (await import('/tests/http.browser.ts')).prepareReloadDownload());
  await page.reload();
  await page.evaluate(async saved => (await import('/tests/http.browser.ts')).checkReloadDownload(saved), saved);
});

test('cross-tab checkpoint locks and real reload recovery @storage', async ({ page, context, origin }) => {
  await page.goto(origin);
  const recovery = await page.evaluate(async () => (await import('/tests/checkpoint.browser.ts')).prepareReload());
  const other = await context.newPage(); await other.goto(origin);
  await other.evaluate(async options => (await import('/tests/checkpoint.browser.ts')).contend(options.namespace, options.checkpointId), recovery);
  await other.close(); await page.reload();
  await page.evaluate(async options => (await import('/tests/checkpoint.browser.ts')).checkReload(options), recovery);
});
test('completed cache reopen and active worker reload @storage', async ({ page, origin }) => {
  await page.goto(origin);
  const cached = await page.evaluate(async () => (await import('/tests/opfs-cache.browser.ts')).createForReload());
  await page.reload();
  expect(await page.evaluate(async id => (await import('/tests/opfs-cache.browser.ts')).checkAfterReload(id), cached.id)).toBe(cached.targetId);
  await page.evaluate(async id => (await import('/tests/opfs-cache.browser.ts')).remove(id), cached.id);
  const partial = await page.evaluate(async () => (await import('/tests/opfs-cache.browser.ts')).holdPartial());
  await page.reload();
  await page.evaluate(async id => (await import('/tests/opfs-cache.browser.ts')).resumeAfterReload(id), partial);
  await page.evaluate(async id => (await import('/tests/opfs-cache.browser.ts')).remove(id), partial);
});
test('cross-tab file lease and stable target after reload @storage', async ({ page, context, origin }) => {
  await page.goto(origin);
  const other = await context.newPage(); await other.goto(origin);
  const name = `q2q-tabs-${crypto.randomUUID()}`;
  const id = await page.evaluate(async name => (await import('/tests/file-destination.browser.ts')).holdFile(name), name);
  expect(await other.evaluate(async name => (await import('/tests/file-destination.browser.ts')).contendFile(name), name)).toBe(id);
  await page.evaluate(async () => (await import('/tests/file-destination.browser.ts')).releaseFile());
  expect(await other.evaluate(async name => (await import('/tests/file-destination.browser.ts')).reopenFile(name), name)).toBe(id);
  await page.reload();
  expect(await page.evaluate(async name => (await import('/tests/file-destination.browser.ts')).reopenFile(name), name)).toBe(id);
  await other.evaluate(async name => (await import('/tests/file-destination.browser.ts')).removeFile(name), name);
});
test('state close stops idle IO before site-data clearing and invalidates recovery @storage', async ({ page, context, browserName, origin }) => {
  test.skip(browserName !== 'chromium', 'Chromium CDP clearing command; other engines retain the common lifecycle tests');
  await page.goto(origin);
  await page.evaluate(async () => (await import('/tests/site-data.browser.ts')).prepare());
  await page.evaluate(async () => (await import('/tests/site-data.browser.ts')).stopState());
  await expect.poll(() => page.evaluate(async () => (await import('/tests/site-data.browser.ts')).ioStopped())).toBe(true);
  const cdp = await context.newCDPSession(page);
  await cdp.send('Storage.clearDataForOrigin', { origin, storageTypes: 'indexeddb,file_systems' });
  await page.evaluate(async () => (await import('/tests/site-data.browser.ts')).check());
});

test('Vite package, Worker and Wasm under CSP @bundle', async ({ browserName }, testInfo) => {
  test.skip(browserName === 'firefox', 'Firefox host environment not yet validated');
  const result = await runBundleStorage({ engine: browserName });
  expect(result.workerAndWasmUnderCsp).toBe(true);
  await testInfo.attach('bundle-result', { body: JSON.stringify(result, null, 2), contentType: 'application/json' });
});
test('100 MiB and 1 GiB disk profile, restart, export and quota @large', async ({ browserName }, testInfo) => {
  test.skip(browserName !== 'chromium', 'RSS and quota measurement currently use Chromium CDP');
  test.setTimeout(180000);
  const path = testInfo.outputPath('large-storage.json');
  const result = await runLargeStorage({ reportPath: path });
  expect(result.cases).toHaveLength(2);
  expect(result.quota.error).toBe('STORAGE_FULL');
  await testInfo.attach('large-storage', { body: await readFile(path), contentType: 'application/json' });
});
