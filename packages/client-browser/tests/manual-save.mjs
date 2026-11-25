import { chromium } from 'playwright-core';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import assert from 'node:assert/strict';
import { startServer } from './browser-server.mjs';

let receiveResult;
const postedResult = new Promise(resolve => { receiveResult = resolve; });
const { server, origin } = await startServer({ onManualResult: receiveResult });
const directory = await mkdtemp(join(tmpdir(), 'q2q-system-save-'));
const target = join(directory, 'saved.bin');
const profile = join(directory, 'profile');
let context;
try {
  let page;
  if (process.env.MANUAL_BROWSER !== '1') {
    context = await chromium.launchPersistentContext(profile, { headless: false,
      ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}) });
    page = await context.newPage();
    await page.goto(origin + '/manual-save');
  }
  console.log(`SAVE_TEST_DIRECTORY=${directory}`);
  console.log(`SAVE_TEST_URL=${origin}/manual-save`);
  console.log('Waiting for native Save dialog selection; choose saved.bin in the test directory above.');
  const result = await postedResult;
  assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(result.kind, 'user-file');
  const actual = await readFile(target);
  const expected = Buffer.alloc(65553);
  for (let index = 0; index < expected.length; index++) expected[index] = (index * 13 + 7) % 251;
  assert.deepEqual(actual, expected);
  assert.equal(createHash('sha256').update(actual).digest('hex'), result.hash);
  console.log(JSON.stringify({ ...result, independentOsFileReadMatches: true }));
} finally {
  await context?.close();
  await new Promise(resolve => server.close(resolve));
  await rm(directory, { recursive: true, force: true });
}
