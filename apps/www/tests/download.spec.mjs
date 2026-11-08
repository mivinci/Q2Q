import { test as base, expect, chromium, webkit } from '@playwright/test';
import { createServer } from 'node:http';
import { readFileSync } from 'node:fs';
import { readFile, mkdir, rm } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { initSync, SeedBuilder, seed_with_source } from '../../../crates/q2q-wasm/pkg/q2q_wasm.js';
initSync({module:readFileSync(new URL('../../../crates/q2q-wasm/pkg/q2q_wasm_bg.wasm',import.meta.url))});
const payload = Buffer.alloc(2 * 1024 * 1024 + 123);
for(let i=0;i<payload.length;i++) payload[i]=(i*31+Math.floor(i/262144))%251;
const test = base.extend({
  context:async ({browserName},use,testInfo) => {
    const profile=testInfo.outputPath('profile');await mkdir(profile,{recursive:true});
    const context=await {chromium,webkit}[browserName].launchPersistentContext(profile,{
      headless:true,baseURL:'http://127.0.0.1:5180',acceptDownloads:true,viewport:{width:1280,height:800},
      ...(browserName==='chromium'&&process.env.CHROME_PATH ? {executablePath:process.env.CHROME_PATH} : {})
    });
    await context.route(/^https:\/\/track\.(?:test\.invalid|q2q\.app)\//,route=>route.fulfill({status:503,json:{code:'test-http-only'}}));
    try {await use(context);} finally {
      await context.close();await rm(profile,{recursive:true,force:true});
    }
  },
  source:[async ({},use) => {
    const requests = new Map();
    const held = new Map(), released = new Set();
    const server = createServer((req,res) => {
      const url = new URL(req.url,'http://localhost');
      const key = url.searchParams.get('case');
      const ranges = requests.get(key) ?? []; requests.set(key,ranges);
      if (url.pathname === '/denied') {res.writeHead(401,{'Access-Control-Allow-Origin':'*'}).end();return;}
      res.setHeader('Access-Control-Allow-Origin','*');res.setHeader('Access-Control-Expose-Headers','Content-Range');
      const match = /^bytes=(\d+)-(\d+)$/.exec(req.headers.range ?? '');
      if (!match) {res.writeHead(400).end();return;}
      const start=Number(match[1]),end=Number(match[2]);ranges.push(start);
      res.writeHead(206,{'Content-Range':`bytes ${start}-${end}/${payload.length}`});
      const bytes=Buffer.from(payload.subarray(start,end+1));
      if(url.pathname==='/corrupt')bytes[0]^=255;
      const send=()=>{if(!res.destroyed)res.end(bytes);};
      if(url.pathname==='/held' && start>0 && !released.has(key)) {
        // Keep the task incomplete until cancellation is observed, independent of UI speed.
        const waiting=held.get(key) ?? new Set();held.set(key,waiting);waiting.add(send);
        res.once('close',()=>{waiting.delete(send);if(!waiting.size)held.delete(key);});
      } else if(url.pathname==='/slow')setTimeout(send,200);else send();
    });
    await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
    const origin=`http://127.0.0.1:${server.address().port}`;
    try { await use({
      seed(kind='good',name='Transfer.bin',changed=false) {
        const key=crypto.randomUUID();
        const content=Buffer.from(payload);if(changed)content[0]^=255;
        const builder=new SeedBuilder(content.length);builder.update(content);
        const material=builder.finish_inline(name);const raw=material.seed_bytes();material.free();
        const bytes=seed_with_source(raw,`${origin}/${kind}?case=${key}&token=private-fixture`);
        return {file:{name:'transfer.q2q',mimeType:'application/octet-stream',buffer:Buffer.from(bytes)},key};
      }, requests,
      release(key) {released.add(key);for(const send of held.get(key) ?? [])send();held.delete(key);}
    }); } finally { await new Promise(resolve=>server.close(resolve)); }
  },{scope:'worker'}]
});
async function upload(page,file) {
  await page.locator('input[type=file][accept]').setInputFiles(file);
  await expect(page.getByRole('dialog').filter({has:page.getByRole('heading',{name:'准备下载',exact:true})})).toBeVisible();
  await page.getByRole('button',{name:'开始下载',exact:true}).click();
}
async function ready(page) {
  await page.goto('/');await expect(page.getByRole('button',{name:'打开种子',exact:true})).toBeEnabled();
}
test('real HTTP download verifies bytes, exports through browser downloads, and deletes cached history',async ({page,source})=>{
  // Exercise the bounded fallback used by browsers without the native picker.
  await page.addInitScript(()=>Object.defineProperty(window,'showSaveFilePicker',{value:undefined,configurable:true}));
  await ready(page); const seed=source.seed(); await upload(page,seed.file);
  const row=page.getByRole('article',{name:'Transfer.bin',exact:true});
  await expect(row.getByText('待保存',{exact:true})).toBeVisible();
  const history=await page.evaluate(()=>JSON.stringify(Object.entries(localStorage)));
  expect(history).not.toContain('private-fixture');expect(history).not.toContain('http://127.0.0.1');
  const downloadPromise=page.waitForEvent('download');
  await row.getByRole('button',{name:'保存文件',exact:true}).click();
  const download=await downloadPromise;
  const saved=await readFile(await download.path());
  expect(createHash('sha256').update(saved).digest('hex')).toBe(createHash('sha256').update(payload).digest('hex'));
  await expect(row.getByText('已提交浏览器保存',{exact:true}).first()).toBeVisible();
  await row.getByRole('button',{name:/详情/}).click();
  await page.getByRole('menuitem',{name:'删除暂存',exact:true}).click();
  await page.getByRole('dialog').getByRole('button',{name:'删除暂存',exact:true}).click();
  await expect(page.getByRole('article')).toHaveCount(0);
  expect(await page.evaluate(()=>Object.keys(localStorage).filter(k=>k.startsWith('q2q-web-transfer-v1:')).length)).toBe(0);
});
test('pause and actual reload resume from checkpoint after reselecting original seed',async ({page,source})=>{
  await ready(page);const seed=source.seed('slow');await upload(page,seed.file);
  const row=page.getByRole('article',{name:'Transfer.bin',exact:true});
  await expect.poll(()=>source.requests.get(seed.key)?.length ?? 0).toBeGreaterThan(2);
  await row.getByRole('button',{name:'暂停',exact:true}).click();
  await expect(row.getByText('已暂停',{exact:true})).toBeVisible();
  const metadata=await page.evaluate(()=>JSON.parse(Object.entries(localStorage).find(([k])=>k.startsWith('q2q-web-transfer-v1:'))[1]));
  expect(BigInt(metadata.committed)).toBeGreaterThan(0n);
  await page.reload();await expect(page.getByText('等待恢复',{exact:true})).toBeVisible();
  await row.getByRole('button',{name:'继续',exact:true}).click();
  await page.locator('input[type=file][accept]').setInputFiles(source.seed('good','Different.bin',true).file);
  await expect(page.getByRole('alert')).toContainText('种子与此任务不匹配');
  await row.getByRole('button',{name:'继续',exact:true}).click();
  const before=source.requests.get(seed.key).length;
  await upload(page,seed.file);await expect(row.getByText('待保存',{exact:true})).toBeVisible();
  expect(source.requests.get(seed.key).slice(before)).not.toContain(0);
});
test('invalid seed, corrupt content and expired authorization have distinct errors',async ({page,source})=>{
  await ready(page);
  await page.locator('input[type=file][accept]').setInputFiles({name:'bad.q2q',mimeType:'application/octet-stream',buffer:Buffer.from('invalid seed')});
  await expect(page.getByRole('alert')).toContainText('无法读取这个种子');
  await upload(page,source.seed('corrupt','Corrupt.bin').file);
  await expect(page.getByRole('article',{name:'Corrupt.bin'}).getByRole('alert')).toContainText('来源内容与种子不一致');
  await page.locator('input[type=file][accept]').setInputFiles(source.seed('denied','Denied.bin').file);
  await page.getByRole('button',{name:'开始下载',exact:true}).click();
  await expect(page.getByRole('article',{name:'Denied.bin'}).getByRole('alert')).toContainText('下载授权已过期');
});
test('settings preserve language after reload, dark mode and mobile layout have no overflow',async ({page})=>{
  await page.setViewportSize({width:390,height:844});await ready(page);
  await page.getByRole('button',{name:'设置',exact:true}).click();
  await page.getByRole('button',{name:'深色',exact:true}).click();
  await expect(page.locator('html')).toHaveClass(/dark/);
  await page.getByRole('button',{name:'语言: 简体中文',exact:true}).click();
  await page.getByRole('menuitemradio',{name:'English',exact:true}).click();
  await page.keyboard.press('Escape');await page.reload();
  await expect(page.getByRole('button',{name:'Open seed',exact:true})).toBeEnabled();
  await expect(page.locator('html')).toHaveAttribute('lang','en');
  expect(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth)).toBe(false);
});

test('cancel stops requests and retry creates a new SDK task using committed progress',async ({page,source})=>{
  await ready(page);const seed=source.seed('held','Cancel.bin');await upload(page,seed.file);
  const row=page.getByRole('article',{name:'Cancel.bin',exact:true});
  await expect.poll(()=>source.requests.get(seed.key)?.length ?? 0).toBeGreaterThan(2);
  await row.getByRole('button',{name:/详情/}).click();
  await page.getByRole('menuitem',{name:'取消下载',exact:true}).click();
  await expect(row.getByText('已取消',{exact:true})).toBeVisible();
  const count=source.requests.get(seed.key).length;
  await page.waitForTimeout(450);
  expect(source.requests.get(seed.key).length).toBe(count);
  const committed=await page.evaluate(()=>JSON.parse(Object.entries(localStorage).find(([key])=>key.startsWith('q2q-web-transfer-v1:'))[1]).committed);
  expect(BigInt(committed)).toBeGreaterThan(0n);
  source.release(seed.key);
  await row.getByRole('button',{name:'重试',exact:true}).click();
  await expect(row.getByText('待保存',{exact:true})).toBeVisible();
  expect(source.requests.get(seed.key).slice(count)).not.toContain(0);
});

test('independent tabs keep both transfer records without overwriting history',async ({page,context,source})=>{
  await ready(page);const first=source.seed('slow','First.bin');await upload(page,first.file);
  await expect.poll(()=>source.requests.get(first.key)?.length ?? 0).toBeGreaterThan(0);
  const second=await context.newPage();await ready(second);
  await upload(second,source.seed('good','Second.bin').file);
  await expect(second.getByRole('article',{name:'Second.bin',exact:true}).getByText('待保存',{exact:true})).toBeVisible();
  await expect(page.getByRole('article',{name:'First.bin',exact:true}).getByText('待保存',{exact:true})).toBeVisible();
  await page.reload();
  await expect(page.getByRole('article')).toHaveCount(2);
  expect(await page.evaluate(()=>Object.keys(localStorage).filter(k=>k.startsWith('q2q-web-transfer-v1:')).length)).toBe(2);
  await second.close();
});

test('browser-enforced storage quota reports actionable space failure and leaves cache removable',async({page,context,source,browserName})=>{
 test.skip(browserName!=='chromium','Storage quota override requires Chromium CDP');
 await ready(page);const cdp=await context.newCDPSession(page);
 await cdp.send('Storage.overrideQuotaForOrigin',{origin:'http://127.0.0.1:5180',quotaSize:1024*1024});
 try{
  await upload(page,source.seed().file);
  await expect(page.getByRole('alert').filter({hasText:'本地空间不足'})).toBeVisible({timeout:15000});
  const row=page.getByRole('article',{name:'Transfer.bin',exact:true});
  if(await row.count()){
   await row.getByRole('button',{name:/详情/}).click();await page.getByRole('menuitem',{name:'删除暂存',exact:true}).click();
   await page.getByRole('dialog').getByRole('button',{name:'删除暂存',exact:true}).click();await expect(row).toHaveCount(0);
  }
 }finally{await cdp.send('Storage.overrideQuotaForOrigin',{origin:'http://127.0.0.1:5180'});await cdp.detach();}
});
