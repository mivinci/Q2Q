import {test as base,expect,chromium,webkit} from '@playwright/test';
import {mkdir,rm,readFile} from 'node:fs/promises';
import {readFileSync} from 'node:fs';
import {createHash} from 'node:crypto';
import {initSync,decode_seed} from '../../../crates/q2q-wasm/pkg/q2q_wasm.js';
import {networkFixture} from '../../../packages/client-web/tests/network-fixture.mjs';
import {p2pFixture} from '../../../packages/client-web/tests/p2p-fixture.mjs';
initSync({module:readFileSync(new URL('../../../crates/q2q-wasm/pkg/q2q_wasm_bg.wasm',import.meta.url))});
const payload=Buffer.alloc(3*262144+17);for(let i=0;i<payload.length;i++)payload[i]=(i*31+Math.floor(i/262144))%251;
const hash=createHash('sha256').update(payload).digest('hex');
const test=base.extend({
 context:async({browserName},use,testInfo)=>{
  const profile=testInfo.outputPath('profile');await mkdir(profile,{recursive:true});
  const context=await {chromium,webkit}[browserName].launchPersistentContext(profile,{headless:true,baseURL:'http://127.0.0.1:5180',acceptDownloads:true,viewport:{width:1280,height:800},...(browserName==='chromium'?{args:['--use-mock-keychain']}:{}),
   ...(browserName==='chromium'&&process.env.CHROME_PATH?{executablePath:process.env.CHROME_PATH}:{}),
   // Clipboard permissions are Chromium-only; WebKit rejects the names and
   // the tests use the manual-copy fallback there instead.
   ...(browserName==='chromium'?{permissions:['clipboard-read','clipboard-write']}:{})});
  await context.addInitScript(()=>Object.defineProperty(window,'showSaveFilePicker',{value:undefined,configurable:true}));
  try{await use(context);}finally{await context.close();await rm(profile,{recursive:true,force:true});}
 }
});
/** Context-level routing covers every page, including ones opened later. */
async function routeTrack(context,trackUrl){await context.route(/^https:\/\/track\.(?:test\.invalid|q2q\.app)\//,async route=>{
 const url=new URL(route.request().url());const response=await route.fetch({url:trackUrl+url.pathname+url.search});await route.fulfill({response});
});}
async function closeServices(context,services){
 try{await context.unrouteAll({behavior:'wait'});}finally{await services.close();}
}
async function openDownloadDialog(page){
 await page.goto('/');await expect(page.getByRole('button',{name:'下载',exact:true})).toBeEnabled();
 await page.getByRole('button',{name:'下载',exact:true}).click();
 const dialog=page.getByRole('dialog');await expect(dialog).toBeVisible();return dialog;
}
const MAGNET_LABEL='粘贴磁力链 magnet:?xt=urn:sha256:…';
async function pasteLink(dialog,link){
 await dialog.getByLabel(MAGNET_LABEL).fill(link);
 await dialog.getByRole('button',{name:'获取并下载',exact:true}).click();
}
async function pasteMagnet(page,link){
 const dialog=await openDownloadDialog(page);
 await pasteLink(dialog,link);
 try{await expect(page.getByRole('heading',{name:'准备下载',exact:true})).toBeVisible({timeout:30000});}
 catch(error){
  // Capture the live dialog before teardown navigates to about:blank.
  const alerts=await page.getByRole('alert').allTextContents().catch(()=>[]);
  await test.info().attach('magnet-resolve-failure',{body:JSON.stringify({alerts}),contentType:'application/json'});
  await test.info().attach('magnet-resolve-page',{body:await page.screenshot(),contentType:'image/png'});
  throw new Error(`Magnet preview did not open: ${JSON.stringify(alerts)}`,{cause:error});
 }
 await page.getByRole('button',{name:'开始下载',exact:true}).click();
}
async function exportAndVerify(page,row){
 await expect(row.getByText('待保存',{exact:true})).toBeVisible({timeout:30000});
 const result=page.waitForEvent('download');
 await row.getByRole('button',{name:'保存文件',exact:true}).click();const download=await result;
 expect(createHash('sha256').update(await readFile(await download.path())).digest('hex')).toBe(hash);
}
async function startSharing(page){
 await page.goto('/');await expect(page.getByRole('button',{name:'分享文件',exact:true})).toBeEnabled();
 await page.locator('input[type=file]:not([accept])').setInputFiles({name:'published.bin',mimeType:'application/octet-stream',buffer:payload});
 await expect(page.getByRole('heading',{name:'准备分享',exact:true})).toBeVisible();
 await page.getByRole('button',{name:'开始分享',exact:true}).click();
 const row=page.getByRole('article',{name:'分享: published.bin',exact:true});
 await expect(row.getByText('等待接收',{exact:true})).toBeVisible();
 await expect(row.getByRole('button',{name:'保存种子',exact:true})).toBeEnabled();return row;
}
async function copyLink(page,row,{fallback=false}={}){
 // WebKit cannot grant clipboard permissions; force the product's denial path
 // there so the same test covers the manual-copy fallback dialog.
 if(fallback)await page.evaluate(()=>{
   const denied=()=>Promise.reject(new DOMException('denied','NotAllowedError'));
   try{globalThis.Clipboard.prototype.writeText=denied;}
   catch{Object.defineProperty(navigator,'clipboard',{value:{writeText:denied},configurable:true});}
 });
 await row.getByRole('button',{name:/详情/}).click();
 await page.getByRole('menuitem',{name:'复制磁力链',exact:true}).click();
 // Copy is async; the row confirms success, and a clipboard denial falls back
 // to a dialog holding the same link. isVisible() does not wait, so poll.
 const copied=row.getByRole('status').filter({hasText:'已复制'});
 const confirmed=await copied.waitFor({state:'visible',timeout:10000}).then(()=>true,()=>false);
 if(confirmed) return await page.evaluate(()=>navigator.clipboard.readText());
 const dialog=page.getByRole('dialog');
 await expect(dialog).toBeVisible();
 const value=await dialog.locator('input').inputValue();
 await dialog.getByRole('button',{name:'取消',exact:true}).last().click();
 return value;
}
async function stopSharing(page,row){
 await row.getByRole('button',{name:/详情/}).click();
 await page.getByRole('menuitem',{name:'停止分享',exact:true}).click();
}

test('pasted magnet resolves from an online publisher, downloads and exports verified bytes',async({page,context,browserName},testInfo)=>{
 test.setTimeout(90000);const services=await networkFixture();let receiverContext;
 try{
  await routeTrack(context,services.trackUrl);const shared=await startSharing(page);
  const link=await copyLink(page,shared,{fallback:browserName==='webkit'});
  expect(link).toMatch(/^magnet:\?xt=urn:sha256:[a-f0-9]{64}/);
  let receiver;
  if(browserName==='chromium'){
   // Chromium's loopback metadata RTC handshake is unreliable between pages
   // sharing one profile. Use independent browser processes as on two devices.
   const profile=testInfo.outputPath('receiver-profile');await mkdir(profile,{recursive:true});
   receiverContext=await chromium.launchPersistentContext(profile,{
    headless:true,baseURL:'http://127.0.0.1:5180',acceptDownloads:true,args:['--use-mock-keychain'],
    ...(process.env.CHROME_PATH?{executablePath:process.env.CHROME_PATH}:{})});
   await receiverContext.addInitScript(()=>Object.defineProperty(window,'showSaveFilePicker',{value:undefined,configurable:true}));
   await routeTrack(receiverContext,services.trackUrl);
   receiver=await receiverContext.newPage();
  }else{
   receiver=await context.newPage();
  }
  await pasteMagnet(receiver,link);
  const row=receiver.getByRole('article',{name:'published.bin',exact:true}).first();
  await exportAndVerify(receiver,row);
  // History records the magnet origin but never the link itself.
  const records=await receiver.evaluate(()=>Object.entries(localStorage)
    .filter(([key])=>key.startsWith('q2q-web-transfer-v1:'))
    .map(([,value])=>JSON.parse(value)));
  expect(records).toHaveLength(1);
  expect(records[0].origin).toBe('magnet');
  expect(JSON.stringify(records)).not.toContain('magnet:');
  await stopSharing(page,shared);
 }finally{
  if(receiverContext){await receiverContext.unrouteAll({behavior:'wait'});await receiverContext.close();}
  await rm(testInfo.outputPath('receiver-profile'),{recursive:true,force:true});
  await page.goto('about:blank').catch(()=>{});await closeServices(context,services);
 }
});

test('invalid, BitTorrent and unknown links report typed errors without creating tasks',async({page,context})=>{
 const services=await networkFixture();
 try{
  await routeTrack(context,services.trackUrl);
  for(const [link,message] of [
   ['not-a-link','无法识别这个磁力链，请检查后重试。'],
   [`magnet:?xt=urn:sha256:${'z'.repeat(64)}`,'无法识别这个磁力链，请检查后重试。'],
   ['magnet:?xt=btih:0123456789abcdef0123456789abcdef01234567','不支持 BitTorrent 磁力链；Q2Q 使用 urn:sha256 链接。']
  ]){
   const dialog=await openDownloadDialog(page);
   await pasteLink(dialog,link);
   await expect(dialog.getByRole('alert')).toHaveText(message);
   await dialog.getByRole('button',{name:'返回',exact:true}).click();
   await expect(page.getByRole('dialog')).toHaveCount(0);
  }
  // A well-formed link nobody serves ends in an explicit no-source error.
  const dialog=await openDownloadDialog(page);
  await pasteLink(dialog,`magnet:?xt=urn:sha256:${'ab'.repeat(32)}`);
  await expect(dialog.getByText('正在查找在线来源')).toBeVisible();
  await expect(dialog.getByRole('alert')).toHaveText('此内容没有可用来源，请确认分享方在线后重试。',{timeout:30000});
  await expect(page.getByRole('article')).toHaveCount(0);
 }finally{await page.goto('about:blank').catch(()=>{});await closeServices(context,services);}
});

test('resolving is cancellable and Esc closes the dialog without a task',async({page,context})=>{
 test.setTimeout(60000);const services=await networkFixture();
 try{
  await routeTrack(context,services.trackUrl);
  const dialog=await openDownloadDialog(page);
  await pasteLink(dialog,`magnet:?xt=urn:sha256:${'cd'.repeat(32)}`);
  await expect(dialog.getByText('正在查找在线来源')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(page.getByRole('article')).toHaveCount(0);
  // The magnet resolve was abandoned, not left running.
  await expect(page.getByRole('button',{name:'下载',exact:true})).toBeEnabled();
 }finally{await page.goto('about:blank').catch(()=>{});await closeServices(context,services);}
});

test('cached metadata restores a magnet task after reload without re-pasting the seed',async({page,context,browserName})=>{
 test.skip(browserName==='webkit','WebKit/native ICE remains unverified');
 // A rate-limited native publisher keeps the transfer mid-flight, so pausing
 // and reloading stays deterministic instead of racing a loopback-speed finish.
 test.setTimeout(120000);const services=await p2pFixture();let receiver;
 try{
  await routeTrack(context,services.trackUrl);
  const infoHash=JSON.parse(decode_seed(new Uint8Array(Buffer.from(services.seed,'base64')))).infoHash;
  const link=`magnet:?xt=urn:sha256:${infoHash}`;
  await pasteMagnet(page,link);
  const row=page.getByRole('article',{name:'browser-p2p.bin',exact:true}).first();
  await expect(row.getByRole('button',{name:'暂停',exact:true})).toBeEnabled({timeout:30000});
  await row.getByRole('button',{name:'暂停',exact:true}).click();
  await expect(row.getByText('已暂停',{exact:true})).toBeVisible();
  // The task record must persist before reload; blocks already committed are
  // only an optimization hint, so only the record is required here.
  await expect.poll(()=>page.evaluate(()=>Object.entries(localStorage).filter(([key])=>key.startsWith('q2q-web-transfer-v1:')).length)).toBe(1);
  await page.reload();
  const restored=page.getByRole('article',{name:'browser-p2p.bin',exact:true});
  await expect(restored.getByText('等待恢复',{exact:true})).toBeVisible();
  await expect(restored.getByText('重新粘贴磁力链后，将校验暂存内容并继续下载。',{exact:true})).toBeVisible();
  await restored.getByRole('button',{name:'重新粘贴链接',exact:true}).click();
  const dialog=page.getByRole('dialog');
  await pasteLink(dialog,link);
  await expect(page.getByRole('heading',{name:'准备下载',exact:true})).toBeVisible({timeout:30000});
  await page.getByRole('button',{name:'开始下载',exact:true}).click();
  await expect(restored.getByText('待保存',{exact:true})).toBeVisible({timeout:90000});
  await services.finish();
 }finally{await receiver?.goto('about:blank').catch(()=>{});await receiver?.close();await page.goto('about:blank').catch(()=>{});await closeServices(context,services);}
});

test('native publisher seed also yields a pasteable magnet with verified bytes',async({page,context,browserName})=>{
 test.skip(browserName==='webkit','WebKit/native ICE remains unverified');test.setTimeout(90000);
 const services=await p2pFixture();
 try{
  await routeTrack(context,services.trackUrl);
  // The minimal xt-only form is the documented common case: identity comes
  // from the verified info, and the page's configured Track does discovery.
  const seedBytes=Buffer.from(services.seed,'base64');
  const infoHash=JSON.parse(decode_seed(new Uint8Array(seedBytes))).infoHash;
  const link=`magnet:?xt=urn:sha256:${infoHash}`;
  await pasteMagnet(page,link);
  const row=page.getByRole('article',{name:'browser-p2p.bin',exact:true}).first();
  await expect(row.getByText('待保存',{exact:true})).toBeVisible({timeout:60000});
  const result=page.waitForEvent('download');
  await row.getByRole('button',{name:'保存文件',exact:true}).click();const download=await result;
  // The native bridge publishes the same fixture bytes this spec hashes.
  expect(createHash('sha256').update(await readFile(await download.path())).digest('hex')).toBe(hash);
  await services.finish();
 }finally{await page.goto('about:blank').catch(()=>{});await closeServices(context,services);}
});

test('magnet dialog is keyboard reachable and fits a narrow viewport',async({page})=>{
 await page.setViewportSize({width:320,height:844});
 await page.goto('/');await expect(page.getByRole('button',{name:'下载',exact:true})).toBeEnabled();
 await page.getByRole('button',{name:'下载',exact:true}).focus();await page.keyboard.press('Enter');
 const input=page.getByLabel(MAGNET_LABEL);
 await expect(input).toBeFocused();
 expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
 const box=await page.getByRole('dialog').boundingBox();
 expect(box.x).toBeGreaterThanOrEqual(8);expect(box.x+box.width).toBeLessThanOrEqual(320-8);
 await page.keyboard.press('Escape');await expect(page.getByRole('dialog')).toHaveCount(0);
});

test('English UI localizes the magnet path',async({page,context})=>{
 await context.addCookies([{name:'q2q-web-locale',value:'en',url:'http://127.0.0.1:5180'}]);
 await page.goto('/');await expect(page.getByRole('button',{name:'Download',exact:true})).toBeEnabled();
 await page.getByRole('button',{name:'Download',exact:true}).click();
 const dialog=page.getByRole('dialog');
 await dialog.getByLabel('Paste a magnet:?xt=urn:sha256:… link').fill('magnet:?xt=btih:0123456789abcdef0123456789abcdef01234567');
 await dialog.getByRole('button',{name:'Fetch and download',exact:true}).click();
 await expect(dialog.getByRole('alert')).toHaveText('BitTorrent magnets are unsupported; Q2Q uses urn:sha256 links.');
});

test('native SDK resolves and downloads the magnet link exported by the live page',async({page,context,browserName})=>{
 test.skip(browserName==='webkit','WebKit/native ICE remains unverified');test.setTimeout(150000);
 const {execFile}=await import('node:child_process'),{promisify}=await import('node:util'),{fileURLToPath}=await import('node:url');
 const execute=promisify(execFile);
 const root=fileURLToPath(new URL('../../../',import.meta.url));
 const compiled=await execute('cargo',['test','--locked','--offline','-p','q2q-core','--test','sdk','--no-run','--message-format=json'],{cwd:root,maxBuffer:8*1024*1024});
 const artifact=compiled.stdout.split('\n').filter(Boolean).map(line=>JSON.parse(line)).find(row=>row.reason==='compiler-artifact'&&row.target.name==='sdk'&&row.executable);
 if(!artifact)throw Error('Native test binary missing');
 const services=await networkFixture();
 try{
  await routeTrack(context,services.trackUrl);
  const shared=await startSharing(page);
  const link=await copyLink(page,shared);
  expect(link).toMatch(/^magnet:\?xt=urn:sha256:[a-f0-9]{64}/);
  // The native side resolves verified metadata from this page's publication
  // and downloads the full file through the same path the CLI uses.
  const result=await execute(artifact.executable,['native_downloads_web_magnet_link','--ignored','--nocapture'],
    {cwd:root,env:{...process.env,Q2Q_BROWSER_UPLOAD_TRACK:services.trackUrl,Q2Q_BROWSER_UPLOAD_MAGNET:link},timeout:90000});
  expect(result.stdout).toContain('Browser publication verified by native SDK');
  await stopSharing(page,shared);
 }finally{await page.goto('about:blank').catch(()=>{});await closeServices(context,services);}
});

test('HTTP-only build reports that magnet downloads need P2P',async({page})=>{
 test.setTimeout(120000);
 const {spawn}=await import('node:child_process');
 const {fileURLToPath}=await import('node:url');
 const root=fileURLToPath(new URL('../../../apps/www/',import.meta.url));
 const child=spawn('npm',['run','dev','--','--port','5181'],{cwd:root,stdio:'ignore',
  env:{...process.env,PUBLIC_Q2Q_TRACK_URL:'',...(process.env.WWW_DEV_HOME?{HOME:process.env.WWW_DEV_HOME}:{})}});
 try{
  let up=false;
  for(let attempt=0;attempt<120&&!up;attempt++){
   await new Promise(resolve=>setTimeout(resolve,500));
   try{const response=await fetch('http://127.0.0.1:5181/');up=response.ok;}catch{/* not listening yet */}
  }
  if(!up)throw Error('HTTP-only dev server did not start');
  await page.goto('http://127.0.0.1:5181/');
  await expect(page.getByRole('button',{name:'下载',exact:true})).toBeEnabled();
  await page.getByRole('button',{name:'下载',exact:true}).click();
  const dialog=page.getByRole('dialog');
  await dialog.getByLabel(MAGNET_LABEL).fill(`magnet:?xt=urn:sha256:${'ab'.repeat(32)}`);
  await dialog.getByRole('button',{name:'获取并下载',exact:true}).click();
  // Parsing is local and succeeds; discovery has no Track, so the dialog must
  // name the missing P2P capability rather than a generic network failure.
  await expect(dialog.getByRole('alert')).toHaveText('磁力链下载需要 P2P 网络，当前构建未启用。',{timeout:30000});
 }finally{
  child.kill('SIGTERM');
  await new Promise(resolve=>{if(child.exitCode!==null)resolve();else{child.once('exit',resolve);setTimeout(resolve,5000);}});
 }
});
