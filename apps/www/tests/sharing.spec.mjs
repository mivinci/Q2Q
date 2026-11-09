import {test as base,expect,chromium,webkit} from '@playwright/test';
import {mkdir,rm,readFile,writeFile,mkdtemp} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {networkFixture} from '../../../packages/client-web/tests/network-fixture.mjs';
import {p2pFixture} from '../../../packages/client-web/tests/p2p-fixture.mjs';
const payload=Buffer.alloc(3*262144+17);for(let i=0;i<payload.length;i++)payload[i]=(i*31+Math.floor(i/262144))%251;
const hash=createHash('sha256').update(payload).digest('hex');
const test=base.extend({
 context:async({browserName},use,testInfo)=>{
  const profile=testInfo.outputPath('profile');await mkdir(profile,{recursive:true});
  const context=await {chromium,webkit}[browserName].launchPersistentContext(profile,{headless:true,baseURL:'http://127.0.0.1:5180',acceptDownloads:true,viewport:{width:1280,height:800},
   ...(browserName==='chromium'&&process.env.CHROME_PATH?{executablePath:process.env.CHROME_PATH}:{})});
  await context.addInitScript(()=>Object.defineProperty(window,'showSaveFilePicker',{value:undefined,configurable:true}));
  try{await use(context);}finally{await context.close();await rm(profile,{recursive:true,force:true});}
 }
});
async function routeTrack(context,trackUrl){await context.route(/^https:\/\/track\.(?:test\.invalid|q2q\.app)\//,async route=>{
 const url=new URL(route.request().url());const response=await route.fetch({url:trackUrl+url.pathname+url.search});await route.fulfill({response});
});}
async function closeServices(context,services){
 // route.fetch uses a separate HTTP client and can outlive page navigation.
 // Drain forwarding handlers while their local Track service is still available.
 try{await context.unrouteAll({behavior:'wait'});}finally{await services.close();}
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
async function seedFrom(page,row){const result=page.waitForEvent('download');await row.getByRole('button',{name:'保存种子',exact:true}).click();const download=await result;return await readFile(await download.path());}
async function receive(page,seed,name='published.bin'){
 await page.goto('/');await expect(page.getByRole('button',{name:'打开种子',exact:true})).toBeEnabled();
 const count=await page.getByRole('article',{name,exact:true}).count();
 await page.locator('input[type=file][accept]').setInputFiles({name:'publication.q2q',mimeType:'application/octet-stream',buffer:seed});
 await page.getByRole('button',{name:'开始下载',exact:true}).click();
 await expect(page.getByRole('article',{name,exact:true})).toHaveCount(count+1);
 const row=page.getByRole('article',{name,exact:true}).first();await expect(row.getByText('待保存',{exact:true})).toBeVisible({timeout:30000});
 const result=page.waitForEvent('download');await row.getByRole('button',{name:'保存文件',exact:true}).click();const download=await result;
 expect(createHash('sha256').update(await readFile(await download.path())).digest('hex')).toBe(hash);
}
async function stop(page,row){await row.getByRole('button',{name:/详情/}).click();await page.getByRole('menuitem',{name:'停止分享',exact:true}).click();await expect(row.getByText('已停止分享',{exact:true})).toBeVisible();}

test('real page shares a file exports seed and downloads verified P2P bytes in another page',async({page,context})=>{
 const services=await networkFixture();let receiver;
 try{await routeTrack(context,services.trackUrl);const row=await startSharing(page),seed=await seedFrom(page,row);
 receiver=await context.newPage();await receive(receiver,seed);
 expect(await page.evaluate(()=>JSON.stringify(Object.entries(localStorage)))).not.toContain('registry');
 await stop(page,row);
 }finally{await receiver?.close();await page.goto('about:blank');await closeServices(context,services);}
});

test('share pause resume stop and reload have honest foreground-only states on a narrow page',async({page,context})=>{
 const services=await networkFixture();
 try{await routeTrack(context,services.trackUrl);await page.setViewportSize({width:390,height:844});const row=await startSharing(page);
 await row.getByRole('button',{name:/详情/}).click();await page.getByRole('menuitem',{name:'暂停',exact:true}).click();await expect(row.getByText('已暂停',{exact:true})).toBeVisible();
 await row.getByRole('button',{name:/详情/}).click();await page.getByRole('menuitem',{name:'继续',exact:true}).click();await expect(row.getByText('等待接收',{exact:true})).toBeVisible();
 expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
 await stop(page,row);await page.reload();await expect(page.getByRole('article')).toHaveCount(0);
 }finally{await page.goto('about:blank').catch(()=>{});await closeServices(context,services);}
});

test('main page accepts a peer-only native seed and exports identical file bytes',async({page,context,browserName})=>{
 test.skip(browserName==='webkit','WebKit/native ICE remains unverified');test.setTimeout(60000);
 const services=await p2pFixture();
 try{await routeTrack(context,services.trackUrl);await receive(page,Buffer.from(services.seed,'base64'),'browser-p2p.bin');await services.finish();}
 finally{await page.goto('about:blank').catch(()=>{});await closeServices(context,services);}
});

test('downloaded content is shared only after explicit action and active cache cannot be deleted',async({page,context})=>{
 test.setTimeout(90000);const services=await networkFixture();let receiver,third;
 try{await routeTrack(context,services.trackUrl);const originRow=await startSharing(page),originalSeed=await seedFrom(page,originRow);
 receiver=await context.newPage();await receive(receiver,originalSeed);
 await expect(receiver.getByRole('article',{name:'分享: published.bin',exact:true})).toHaveCount(0);
 const row=receiver.getByRole('article',{name:'published.bin',exact:true});
 await row.getByRole('button',{name:/详情/}).click();await receiver.getByRole('menuitem',{name:'继续分享',exact:true}).click();
 const cached=receiver.getByRole('article',{name:'分享: published.bin',exact:true});await expect(cached.getByText('等待接收',{exact:true})).toBeVisible();
 await row.getByRole('button',{name:/详情/}).click();await expect(receiver.getByRole('menuitem',{name:'删除暂存',exact:true})).toHaveAttribute('data-disabled');await receiver.keyboard.press('Escape');
 const seed=await seedFrom(receiver,cached);await stop(page,originRow);
 third=await context.newPage();await receive(third,seed);
 await stop(receiver,cached);
 }finally{await third?.close();await receiver?.goto('about:blank');await receiver?.close();await page.goto('about:blank');await closeServices(context,services);}
});

test('native SDK downloads the seed exported from the real sharing page',async({page,context,browserName})=>{
 test.skip(browserName==='webkit','WebKit/native ICE remains unverified');test.setTimeout(90000);
 const {execFile}=await import('node:child_process'),{promisify}=await import('node:util'),{fileURLToPath}=await import('node:url');const execute=promisify(execFile);
 const root=fileURLToPath(new URL('../../../',import.meta.url));
 const compiled=await execute('cargo',['test','--locked','--offline','-p','q2q-core','--test','sdk','--no-run','--message-format=json'],{cwd:root,maxBuffer:8*1024*1024});
 const artifact=compiled.stdout.split('\n').filter(Boolean).map(line=>JSON.parse(line)).find(row=>row.reason==='compiler-artifact'&&row.target.name==='sdk'&&row.executable);
 if(!artifact)throw Error('Native test binary missing');
 const services=await networkFixture(),folder=await mkdtemp(join(tmpdir(),'q2q-www-native-'));
 try{await routeTrack(context,services.trackUrl);const row=await startSharing(page),seed=await seedFrom(page,row);const path=join(folder,'web.q2q');await writeFile(path,seed);
 const result=await execute(artifact.executable,['native_downloads_browser_publication','--ignored','--nocapture'],{cwd:root,env:{...process.env,Q2Q_BROWSER_UPLOAD_TRACK:services.trackUrl,Q2Q_BROWSER_UPLOAD_SEED:path},timeout:50000});
 expect(result.stdout).toContain('Browser publication verified by native SDK');await stop(page,row);
 }finally{await page.goto('about:blank');await closeServices(context,services);await rm(folder,{recursive:true,force:true});}
});

test('sharing confirmation is cancellable, details do not stop publication, and labels follow language',async({page,context})=>{
 const services=await networkFixture();
 try{await routeTrack(context,services.trackUrl);await page.goto('/');await expect(page.getByRole('button',{name:'分享文件',exact:true})).toBeEnabled();
 await page.locator('input[type=file]:not([accept])').setInputFiles({name:'published.bin',mimeType:'application/octet-stream',buffer:payload});
 await page.getByRole('dialog').getByRole('button',{name:'取消',exact:true}).last().click();await expect(page.getByRole('article')).toHaveCount(0);
 const row=await startSharing(page);await row.getByRole('button',{name:/详情/}).click();await page.getByRole('menuitem',{name:'传输详情',exact:true}).click();
 await expect(page.getByRole('dialog').getByText('请保持页面打开',{exact:false})).toBeVisible();await page.keyboard.press('Escape');
 await expect(row.getByText('等待接收',{exact:true})).toBeVisible();
 await page.getByRole('button',{name:'设置',exact:true}).click();await page.getByRole('button',{name:'语言: 简体中文',exact:true}).click();await page.getByRole('menuitemradio',{name:'English',exact:true}).click();await page.keyboard.press('Escape');
 await expect(page.getByRole('article',{name:'Share: published.bin',exact:true}).getByText('Waiting for receiver',{exact:true})).toBeVisible();
 await expect(page.getByRole('button',{name:'Save seed',exact:true})).toBeEnabled();
 }finally{await page.goto('about:blank').catch(()=>{});await closeServices(context,services);}
});

async function startDownload(page,seed,name='published.bin'){
 await page.goto('/');await expect(page.getByRole('button',{name:'打开种子',exact:true})).toBeEnabled();
 await page.locator('input[type=file][accept]').setInputFiles({name:'original.q2q',mimeType:'application/octet-stream',buffer:seed});
 await page.getByRole('button',{name:'开始下载',exact:true}).click();return page.getByRole('article',{name,exact:true}).first();
}
async function exportAndVerify(page,row){
 await expect(row.getByText('待保存',{exact:true})).toBeVisible({timeout:30000});const result=page.waitForEvent('download');
 await row.getByRole('button',{name:'保存文件',exact:true}).click();const download=await result;
 expect(createHash('sha256').update(await readFile(await download.path())).digest('hex')).toBe(hash);
}

test('offline peer source fails honestly and retry succeeds once the source resumes',async({page,context})=>{
 const services=await networkFixture();let receiver;
 try{await routeTrack(context,services.trackUrl);const shared=await startSharing(page),seed=await seedFrom(page,shared);
 await shared.getByRole('button',{name:/详情/}).click();await page.getByRole('menuitem',{name:'暂停',exact:true}).click();await expect(shared.getByText('已暂停',{exact:true})).toBeVisible();
 receiver=await context.newPage();const row=await startDownload(receiver,seed);
 await expect(row.getByText('下载失败',{exact:true})).toBeVisible({timeout:30000});await expect(row.getByRole('button',{name:'重试',exact:true})).toBeEnabled();
 await shared.getByRole('button',{name:/详情/}).click();await page.getByRole('menuitem',{name:'继续',exact:true}).click();await expect(shared.getByText('等待接收',{exact:true})).toBeVisible();
 await row.getByRole('button',{name:'重试',exact:true}).click();await exportAndVerify(receiver,row);await stop(page,shared);
 }finally{await receiver?.goto('about:blank').catch(()=>{});await receiver?.close();await page.goto('about:blank').catch(()=>{});await closeServices(context,services);}
});

test('lost Punch connection fails the share and a new file selection recovers after service restart',async({page,context})=>{
 test.setTimeout(90000);const services=await networkFixture();
 try{await routeTrack(context,services.trackUrl);const shared=await startSharing(page);await services.stopPunch();
 await expect(shared.getByText('分享失败',{exact:true})).toBeVisible({timeout:15000});
 await services.restartPunch();
 await page.locator('input[type=file]:not([accept])').setInputFiles({name:'recovered.bin',mimeType:'application/octet-stream',buffer:payload});
 await page.getByRole('button',{name:'开始分享',exact:true}).click();
 const recovered=page.getByRole('article',{name:'分享: recovered.bin',exact:true});await expect(recovered.getByText('等待接收',{exact:true})).toBeVisible({timeout:15000});
 await stop(page,recovered);
 }finally{await page.goto('about:blank').catch(()=>{});await closeServices(context,services);}
});

test('peer-only download survives pause and real page reload with seed reselection',async({page,context,browserName})=>{
 test.skip(browserName==='webkit','WebKit/native ICE remains unverified');test.setTimeout(90000);const services=await p2pFixture();
 try{await routeTrack(context,services.trackUrl);const seed=Buffer.from(services.externalSeed,'base64');const row=await startDownload(page,seed,'browser-p2p.bin');
 await expect.poll(()=>page.evaluate(()=>Object.entries(localStorage).filter(([key])=>key.startsWith('q2q-web-transfer-v1:')).map(([,value])=>JSON.parse(value)).some(row=>BigInt(row.committed)>0n))).toBe(true);
 await row.getByRole('button',{name:'暂停',exact:true}).click();await expect(row.getByText('已暂停',{exact:true})).toBeVisible();
 const committed=await page.evaluate(()=>Object.entries(localStorage).filter(([key])=>key.startsWith('q2q-web-transfer-v1:')).map(([,value])=>JSON.parse(value))[0].committed);expect(BigInt(committed)).toBeGreaterThan(0n);
 await page.reload();const restored=page.getByRole('article',{name:'browser-p2p.bin',exact:true});
 await expect(restored.getByText('等待恢复',{exact:true})).toBeVisible();await restored.getByRole('button',{name:'继续',exact:true}).click();
 await page.locator('input[type=file][accept]').setInputFiles({name:'original.q2q',mimeType:'application/octet-stream',buffer:seed});
 await page.getByRole('button',{name:'开始下载',exact:true}).click();await exportAndVerify(page,restored);await services.finish();
 }finally{await page.goto('about:blank').catch(()=>{});await closeServices(context,services);}
});

test('failed preparation does not offer a seed and removing the failure releases a slot',async({page,context})=>{
 const services=await networkFixture();
 try{await routeTrack(context,services.trackUrl);await page.goto('/');await expect(page.getByRole('button',{name:'分享文件',exact:true})).toBeEnabled();
 await context.route(/^https:\/\/track\.(?:test\.invalid|q2q\.app)\//,route=>route.fulfill({status:503,json:{code:'temporary-test-outage'}}));
 await page.locator('input[type=file]:not([accept])').setInputFiles({name:'failure.bin',mimeType:'application/octet-stream',buffer:payload});await page.getByRole('button',{name:'开始分享',exact:true}).click();
 const failed=page.getByRole('article',{name:'分享: failure.bin',exact:true});await expect(failed.getByText('分享失败',{exact:true})).toBeVisible();
 await expect(failed.getByRole('button',{name:'保存种子',exact:true})).toBeDisabled();
 await failed.getByRole('button',{name:/详情/}).click();await page.getByRole('menuitem',{name:'移除任务',exact:true}).click();await expect(failed).toHaveCount(0);
 await context.unroute(/^https:\/\/track\.(?:test\.invalid|q2q\.app)\//);await routeTrack(context,services.trackUrl);
 const row=await startSharing(page);await stop(page,row);
 }finally{await page.goto('about:blank').catch(()=>{});await closeServices(context,services);}
});

test('reloading an active share never restores a fake online task or persisted file access',async({page,context})=>{
 const services=await networkFixture();
 try{await routeTrack(context,services.trackUrl);await startSharing(page);
 expect(await page.evaluate(()=>Object.keys(localStorage).filter(key=>key.startsWith('q2q-web-transfer-v1:')))).toEqual([]);
 page.once('dialog',dialog=>dialog.accept());await page.reload();
 await expect(page.getByRole('button',{name:'分享文件',exact:true})).toBeEnabled();await expect(page.getByRole('article')).toHaveCount(0);
 const row=await startSharing(page);await stop(page,row);
 }finally{await page.goto('about:blank').catch(()=>{});await closeServices(context,services);}
});


test('failed RTC offer exposes safe connection stage in transfer details',async({page,context})=>{
 const services=await networkFixture();let receiver;
 try{
  await routeTrack(context,services.trackUrl);const shared=await startSharing(page),seed=await seedFrom(page,shared);
  receiver=await context.newPage();
  await receiver.addInitScript(()=>{RTCPeerConnection.prototype.createOffer=async()=>{throw new DOMException('private-native-detail','OperationError');};});
  const row=await startDownload(receiver,seed);
  await expect(row.getByText('下载失败',{exact:true})).toBeVisible();
  await row.getByRole('button',{name:/详情/}).click();
  await receiver.getByRole('menuitem',{name:'传输详情',exact:true}).click();
  const diagnostics=receiver.getByRole('dialog').getByRole('region',{name:'连接诊断'});
  await expect(diagnostics).toContainText('PEER_PROTOCOL_ERROR');
  await expect(diagnostics).toContainText('rtc-offer');
  await expect(diagnostics).toContainText('connectionState');
  const text=await diagnostics.innerText();
  for(const secret of ['private-native-detail','127.0.0.1','connect_token','credential','fingerprint'])expect(text).not.toContain(secret);
  expect(await receiver.evaluate(()=>JSON.stringify(Object.entries(localStorage)))).not.toContain('rtc-offer');
  await stop(page,shared);
 }finally{await receiver?.close();await page.goto('about:blank').catch(()=>{});await closeServices(context,services);}
});
