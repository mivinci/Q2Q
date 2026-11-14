import { defineConfig } from '@playwright/test';
import { fileURLToPath } from 'node:url';

export default defineConfig({
  testDir: './tests',
  testMatch: ['storage.spec.mjs','network.spec.mjs','rtc.spec.mjs','p2p.spec.mjs','public-p2p.spec.mjs','publication.spec.mjs','partial.spec.mjs','magnet.spec.mjs'],
  timeout: 45000,
  expect: { timeout: 5000 },
  fullyParallel: true,
  workers: 2,
  retries: 0,
  outputDir: fileURLToPath(new URL('../../.tmp/client-web-playwright/results', import.meta.url)),
  reporter: [
    ['list'],
    ['html', { outputFolder: fileURLToPath(new URL('../../.tmp/client-web-playwright/report', import.meta.url)), open: 'never' }]
  ],
  projects: [
    { name: 'chromium', use: { browserName: 'chromium', ...(process.env.CHROME_PATH ? { launchOptions: {executablePath:process.env.CHROME_PATH} } : {}) } },
    { name: 'webkit', use: { browserName: 'webkit' } },
    ...(process.env.TEST_FIREFOX === '1' ? [{ name: 'firefox', use: { browserName: 'firefox' } }] : [])
  ]
});
