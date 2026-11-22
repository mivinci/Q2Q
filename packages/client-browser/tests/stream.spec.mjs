import { test as base } from '@playwright/test';
import { startServer } from './browser-server.mjs';
import { httpFixture } from './http-fixture.mjs';
import { p2pFixture } from './p2p-fixture.mjs';
const test = base.extend({
  services: async ({ browserName }, use) => {
    if (browserName === 'webkit' && process.env.TEST_NATIVE_WEBKIT !== '1') { await use(null); return; }
    const fixture = await p2pFixture();
    try { await use(fixture); } finally { await fixture.close(); }
  },
  origin: [async ({}, use) => {
    const fixture = httpFixture();
    const { server, origin } = await startServer({ handleRequest: fixture });
    try { await use(origin); } finally { await new Promise(resolve => server.close(resolve)); }
  }, { scope: 'worker' }]
});
for (const name of ['sequentialHeadRead', 'windowAndSeek', 'readTimeoutThenRetry', 'closePreservesVerifiedBlocks', 'peerOnlyRejected', 'argumentBounds']) {
  test(`stream ${name} @stream`, async ({ page, origin }) => {
    await page.goto(origin);
    await page.evaluate(async name => (await import('/tests/stream.browser.ts'))[name](), name);
  });
}

test('stream reads a live peer head and tail @stream @p2p', async ({ page, origin, services, browserName }) => {
  test.skip(browserName === 'webkit' && process.env.TEST_NATIVE_WEBKIT !== '1', 'WebKit/native ICE remains unverified');
  test.setTimeout(90000);
  await page.goto(origin);
  const result = await page.evaluate(async fixture =>
    (await import('/tests/stream.browser.ts')).p2pStream(fixture),
    { seed: services.seed, trackUrl: services.trackUrl });
  test.expect(result.size).toBe(3 * 262144 + 17);
  await services.finish();
});
