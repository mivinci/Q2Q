import { test as base, expect, chromium, webkit } from '@playwright/test';
import { createServer } from 'node:http';
import { readFileSync } from 'node:fs';
import { mkdir, rm } from 'node:fs/promises';
import { initSync, SeedBuilder, seed_with_source } from '../../../crates/q2q-wasm/pkg/q2q_wasm.js';
initSync({ module: readFileSync(new URL('../../../crates/q2q-wasm/pkg/q2q_wasm_bg.wasm', import.meta.url)) });

/** A minimal, genuinely playable PCM WAV so playback is verified for real
 * rather than only structurally. 8 kHz mono, 16-bit, 0.4 s of silence. */
function wav(samples = 3200) {
  const data = Buffer.alloc(samples * 2);
  for (let i = 0; i < samples; i++) data.writeInt16LE(Math.round(Math.sin(i / 12) * 6000), i * 2);
  const header = Buffer.alloc(44);
  header.write('RIFF', 0); header.writeUInt32LE(36 + data.length, 4); header.write('WAVE', 8);
  header.write('fmt ', 12); header.writeUInt32LE(16, 16); header.writeUInt16LE(1, 20); header.writeUInt16LE(1, 22);
  header.writeUInt32LE(8000, 24); header.writeUInt32LE(16000, 28); header.writeUInt16LE(2, 32); header.writeUInt16LE(16, 34);
  header.write('data', 36); header.writeUInt32LE(data.length, 40);
  return Buffer.concat([header, data]);
}

const test = base.extend({
  context: async ({ browserName }, use, testInfo) => {
    const profile = testInfo.outputPath('profile'); await mkdir(profile, { recursive: true });
    const context = await { chromium, webkit }[browserName].launchPersistentContext(profile, {
      headless: true, baseURL: 'http://127.0.0.1:5180', acceptDownloads: true, viewport: { width: 1280, height: 800 },
      ...(browserName === 'chromium' ? { args: ['--use-mock-keychain'] } : {}),
      ...(browserName === 'chromium' && process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {})
    });
    await context.route(/^https:\/\/track\.(?:test\.invalid|q2q\.app)\//, route => route.fulfill({ status: 503, json: { code: 'test-http-only' } }));
    await context.addInitScript(() => Object.defineProperty(window, 'showSaveFilePicker', { value: undefined, configurable: true }));
    try { await use(context); } finally { await context.close(); await rm(profile, { recursive: true, force: true }); }
  },
  source: [async ({}, use) => {
    const payloads = new Map();
    const server = createServer((request, response) => {
      const url = new URL(request.url, 'http://localhost');
      const key = url.searchParams.get('case');
      const bytes = payloads.get(key);
      if (!bytes) { response.writeHead(404).end(); return; }
      response.setHeader('Access-Control-Allow-Origin', '*');
      response.setHeader('Access-Control-Expose-Headers', 'Content-Range');
      const match = /^bytes=(\d+)-(\d+)$/.exec(request.headers.range ?? '');
      if (!match) { response.writeHead(400).end(); return; }
      const start = Number(match[1]), end = Number(match[2]);
      response.writeHead(206, { 'Content-Range': `bytes ${start}-${end}/${bytes.length}` });
      response.end(bytes.subarray(start, end + 1));
    });
    await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
    const origin = `http://127.0.0.1:${server.address().port}`;
    try {
      await use({
        /** Build an inline seed whose content is served over HTTP under `name`. */
        seed(content, name) {
          const key = crypto.randomUUID();
          payloads.set(key, content);
          const builder = new SeedBuilder(content.length); builder.update(content);
          const material = builder.finish_inline(name); const raw = material.seed_bytes(); material.free();
          const bytes = seed_with_source(raw, `${origin}/content?case=${key}`);
          return { name: 'transfer.q2q', mimeType: 'application/octet-stream', buffer: Buffer.from(bytes) };
        }
      });
    } finally { await new Promise(resolve => server.close(resolve)); }
  }, { scope: 'worker' }]
});

/** Downloads a seed and returns the task row, which is named after the content. */
async function download(page, file, contentName) {
  await page.goto('/');
  await expect(page.getByRole('button', { name: '下载', exact: true })).toBeEnabled();
  await page.locator('input[type=file][accept]').setInputFiles(file);
  await expect(page.getByRole('dialog').filter({ has: page.getByRole('heading', { name: '准备下载', exact: true }) })).toBeVisible();
  await page.getByRole('button', { name: '开始下载', exact: true }).click();
  return page.getByRole('article', { name: contentName, exact: true }).first();
}

test('a completed audio file plays from the local copy and keeps the cache on close', async ({ page, source }) => {
  const file = source.seed(wav(), 'tone.wav');
  const row = await download(page, file, 'tone.wav');
  await expect(row.getByText('待保存', { exact: true })).toBeVisible({ timeout: 30000 });

  await row.getByRole('button', { name: '播放', exact: true }).click();
  const dialog = page.getByRole('dialog');
  const audio = dialog.locator('audio');
  await expect(audio).toBeVisible();
  // The URL is a local blob, never a network origin.
  await expect(audio).toHaveAttribute('src', /^blob:/);
  // Real decode: a valid WAV reports duration and loads metadata.
  await expect.poll(() => audio.evaluate(element => element.duration), { timeout: 15000 }).toBeGreaterThan(0.2);
  expect(await audio.evaluate(element => element.readyState)).toBeGreaterThan(0);

  await dialog.getByRole('button', { name: '取消', exact: true }).last().click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  // Closing playback must not delete the verified cache: the row is still ready to save.
  await expect(row.getByText('待保存', { exact: true })).toBeVisible();
  await expect(row.getByRole('button', { name: '保存文件', exact: true })).toBeEnabled();
  expect(await page.evaluate(() => JSON.stringify(Object.entries(localStorage)))).not.toContain('blob:');
});

test('a video-named file opens a video element; a non-media file offers no playback', async ({ page, source }) => {
  // The bytes do not decode as video; this asserts the dialog wiring, not a codec.
  const video = await download(page, source.seed(Buffer.alloc(4096, 7), 'clip.mp4'), 'clip.mp4');
  await expect(video.getByText('待保存', { exact: true })).toBeVisible({ timeout: 30000 });
  await video.getByRole('button', { name: '播放', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.locator('video')).toBeVisible();
  await expect(dialog.locator('video')).toHaveAttribute('src', /^blob:/);
  // The bytes are not a real video, so the browser reports a decode failure.
  // The dialog stays open and explains it instead of vanishing.
  await expect(dialog.getByRole('alert')).toContainText('此浏览器无法直接播放该文件', { timeout: 15000 });
  await dialog.getByRole('button', { name: '取消', exact: true }).last().click();
  await expect(page.getByRole('dialog')).toHaveCount(0);

  const plain = await download(page, source.seed(Buffer.alloc(1024, 3), 'archive.bin'), 'archive.bin');
  await expect(plain.getByText('待保存', { exact: true })).toBeVisible({ timeout: 30000 });
  await expect(plain.getByRole('button', { name: '播放', exact: true })).toHaveCount(0);
});
