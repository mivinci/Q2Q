import { defineConfig } from '@playwright/test';
export default defineConfig({
  testDir:'./tests', timeout:45000, workers:2, fullyParallel:true,
  use:{baseURL:'http://127.0.0.1:5180',trace:'retain-on-failure',screenshot:'only-on-failure'},
  outputDir:'../../.tmp/www-playwright/results',
  reporter:[['list'],['html',{outputFolder:'../../.tmp/www-playwright/report',open:'never'}]],
  // WWW_DEV_HOME only redirects the wrangler/vite dev server's state directory
  // when the host forbids writes outside the workspace; browsers keep the real
  // HOME (a redirected one makes Chromium prompt for a missing Keychain item).
  webServer:{env:{PUBLIC_Q2Q_TRACK_URL:'https://track.test.invalid',...(process.env.WWW_DEV_HOME?{HOME:process.env.WWW_DEV_HOME}:{})},command:process.env.WWW_PREVIEW === '1' ? 'npm run preview -- --port 5180' : 'npm run dev -- --port 5180',url:'http://127.0.0.1:5180',reuseExistingServer:false,timeout:60000},
  projects:[
    {name:'chromium',use:{browserName:'chromium',launchOptions:process.env.CHROME_PATH ? {executablePath:process.env.CHROME_PATH} : {}}},
    {name:'webkit',use:{browserName:'webkit'}}
  ]
});
