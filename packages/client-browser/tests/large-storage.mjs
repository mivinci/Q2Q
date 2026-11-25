import { chromium } from 'playwright-core';
import { execFileSync } from 'node:child_process';
import { mkdir, writeFile, statfs, mkdtemp, rm } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { tmpdir } from 'node:os';
import assert from 'node:assert/strict';
import { startServer } from './browser-server.mjs';

export async function runLargeStorage(options = {}) {
const disk = await statfs(tmpdir());
assert(disk.bavail * disk.bsize > 4 * 1024 ** 3, 'large tests require at least 4 GiB of temporary free space');
const { server, origin } = await startServer();
const report = { browser: '', platform: process.platform, cases: [], quota: null,
  profile: 'fresh on-disk profile, real browser restart after quarter-file commit',
  memoryMethod: '500ms sum of browser process-tree RSS; includes native/Worker/Wasm; shared pages may be counted more than once, short peaks may be missed',
  exportTarget: 'second OPFS file using fileDestination; system save picker is a separate test' };
const launchOptions = { headless: true,
  ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}) };
try {
  for (const mib of [100, 1024]) {
    const profile = await mkdtemp(join(tmpdir(), 'q2q-storage-profile-'));
    let context; let page; let rootPid; let peakRss = 0; let sampleCount = 0; let phase = 'initial';
    async function launch() {
      context = await chromium.launchPersistentContext(profile, launchOptions);
      report.browser = context.browser().version();
      const browserCdp = await context.browser().newBrowserCDPSession();
      const { processInfo } = await browserCdp.send('SystemInfo.getProcessInfo');
      rootPid = processInfo.find(item => item.type === 'browser').id;
      await browserCdp.detach();
      page = await context.newPage();
      await page.goto(origin);
      page.on('console', message => {
        if (message.text().startsWith('C5-PHASE ')) {
          phase = message.text().slice('C5-PHASE '.length);
          console.log(`${mib} MiB: ${phase}`);
        }
      });
    }
    function sample() {
      if (!rootPid || process.platform === 'win32') return null;
      const rows = execFileSync('ps', ['-axo', 'pid=,ppid=,rss='], { encoding: 'utf8' })
        .trim().split('\n').map(row => row.trim().split(/\s+/).map(Number));
      const ids = new Set([rootPid]);
      let changed;
      do { changed = false; for (const [pid, parent] of rows) if (ids.has(parent) && !ids.has(pid)) { ids.add(pid); changed = true; } } while (changed);
      const rss = rows.reduce((sum, [pid, , kb]) => sum + (ids.has(pid) ? kb * 1024 : 0), 0);
      peakRss = Math.max(peakRss, rss); sampleCount++;
      return rss;
    }
    let timer;
    try {
      await launch();
      const baselineRss = sample();
      timer = setInterval(sample, 500);
      const descriptor = await page.evaluate(async mib => (await import('/tests/large-storage.browser.ts')).prepare(mib), mib);
      let cdp = await context.newCDPSession(page);
      const quarterUsage = await cdp.send('Storage.getUsageAndQuota', { origin });
      phase = 'restart-browser';
      await context.close(); rootPid = undefined;
      await launch();
      const result = await page.evaluate(async descriptor => (await import('/tests/large-storage.browser.ts')).resume(descriptor), descriptor);
      sample();
      cdp = await context.newCDPSession(page);
      const finalUsage = await cdp.send('Storage.getUsageAndQuota', { origin });
      const measured = { ...result, baselineRssBytes: baselineRss, peakProcessTreeRssBytes: peakRss,
        sampleCount, quarterOriginUsageBytes: quarterUsage.usage, finalOriginUsageBytes: finalUsage.usage };
      report.cases.push(measured);
      console.log(JSON.stringify(measured));
      await page.evaluate(async () => (await import('/tests/large-storage.browser.ts')).cleanup());
      const afterCleanup = await cdp.send('Storage.getUsageAndQuota', { origin });
      measured.originUsageAfterCleanupBytes = afterCleanup.usage;
    } catch (error) { throw new Error(`${mib} MiB failed in ${phase}`, { cause: error }); }
    finally { clearInterval(timer); await context?.close(); await rm(profile, { recursive: true, force: true }); }
  }
  const profile = await mkdtemp(join(tmpdir(), 'q2q-quota-profile-'));
  const context = await chromium.launchPersistentContext(profile, launchOptions);
  try {
    const page = await context.newPage();
    await page.goto(origin);
    const cdp = await context.newCDPSession(page);
    await cdp.send('Storage.overrideQuotaForOrigin', { origin, quotaSize: 8 * 1024 * 1024 });
    try {
      report.quota = await page.evaluate(async () => (await import('/tests/large-storage.browser.ts')).quotaCase());
      console.log(JSON.stringify({ quota: report.quota }));
    } finally { await cdp.send('Storage.overrideQuotaForOrigin', { origin }); }
  } finally { await context.close(); await rm(profile, { recursive: true, force: true }); }
  const reportPath = options.reportPath ?? process.env.REPORT_PATH;
  if (reportPath) {
    await mkdir(dirname(reportPath), { recursive: true });
    await writeFile(reportPath, JSON.stringify(report, null, 2) + '\n');
  }
  console.log('PASS 100 MiB / 1 GiB checkpoint, browser restart, Rust verification, export and browser-enforced quota');
  return report;
} finally { await new Promise(resolve => server.close(resolve)); }

}
