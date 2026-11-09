import {test as base,expect,chromium,webkit} from '@playwright/test';
import {mkdir,rm} from 'node:fs/promises';
import {createServer} from 'node:http';
import {readFileSync} from 'node:fs';
import {initSync,SeedBuilder,seed_with_source} from '../../../crates/q2q-wasm/pkg/q2q_wasm.js';
import {networkFixture} from '../../../packages/client-web/tests/network-fixture.mjs';
initSync({module:readFileSync(new URL('../../../crates/q2q-wasm/pkg/q2q_wasm_bg.wasm',import.meta.url))});
const bytes=Buffer.alloc(3*1024*1024+17,37);
const test=base.extend({context:async({browserName},use,testInfo)=>{
 const profile=testInfo.outputPath('profile');await mkdir(profile,{recursive:true});
 const context=await {chromium,webkit}[browserName].launchPersistentContext(profile,{headless:true,baseURL:'http://127.0.0.1:5180',acceptDownloads:true,...(browserName==='chromium'&&process.env.CHROME_PATH?{executablePath:process.env.CHROME_PATH}:{})});
 try{await use(context);}finally{await context.close();await rm(profile,{recursive:true,force:true});}
}});
const labels={zh:{share:'分享文件',open:'打开种子',confirm:'准备分享',startShare:'开始分享',waiting:'等待接收',saveSeed:'保存种子',details:'传输详情',pause:'暂停',resume:'继续',paused:'已暂停',startDownload:'开始下载',completed:'待保存',failed:'下载失败',settings:'设置',light:'浅色',dark:'深色',system:'跟随系统',stop:'停止分享'},en:{share:'Share file',open:'Open seed',confirm:'Ready to share',startShare:'Start sharing',waiting:'Waiting for receiver',saveSeed:'Save seed',details:'Transfer details',pause:'Pause',resume:'Resume',paused:'Paused',startDownload:'Start download',completed:'Ready to save',failed:'Download failed',settings:'Settings',light:'Light',dark:'Dark',system:'System',stop:'Stop sharing'}};

for(const width of [320,390,1280])for(const locale of ['zh','en'])for(const theme of ['light','dark'])test(`real UI matrix ${width} ${locale} ${theme}`,async({page,context,browserName},testInfo)=>{
 test.setTimeout(60000);const text=labels[locale],errors=[];page.on('pageerror',error=>errors.push(error.message));
 const services=await networkFixture();
 let slow=true;
 const originServer=createServer((request,response)=>{
  response.setHeader('Access-Control-Allow-Origin','*');response.setHeader('Access-Control-Expose-Headers','Content-Range');
  const range=/bytes=(\d+)-(\d+)/.exec(request.headers.range??'');if(!range){response.writeHead(416).end();return;}
  const start=Number(range[1]),end=Number(range[2]);response.writeHead(206,{'Content-Length':end-start+1,'Content-Range':`bytes ${start}-${end}/${bytes.length}`});
  const send=()=>{if(!response.destroyed)response.end(bytes.subarray(start,end+1));};if(slow)setTimeout(send,200);else send();
 });
 await new Promise(resolve=>originServer.listen(0,'127.0.0.1',resolve));
 const builder=new SeedBuilder(bytes.length);builder.update(bytes);const material=builder.finish_inline('long-untrusted-<img src=x onerror=alert(1)>-文件名-'.repeat(4)+'.bin');
 const seed=seed_with_source(material.seed_bytes(),`http://127.0.0.1:${originServer.address().port}/content`);material.free();
 await context.route(/^https:\/\/track\.(?:test\.invalid|q2q\.app)\//,async route=>{const url=new URL(route.request().url());const response=await route.fetch({url:services.trackUrl+url.pathname+url.search});await route.fulfill({response});});
 await context.addCookies([{name:'q2q-web-locale',value:locale==='en'?'en':'zh-CN',url:'http://127.0.0.1:5180'}]);
 await page.setViewportSize({width,height:width===1280?900:844});await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'});
 async function capture(state){
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`body overflow in ${state}`).toBe(true);
  const dialog=page.getByRole('dialog');if(await dialog.count()){
   const box=await dialog.boundingBox();expect(box.x).toBeGreaterThanOrEqual(8);expect(box.x+box.width).toBeLessThanOrEqual(width-8);
  }
  await page.screenshot({path:testInfo.outputPath(`${width}-${locale}-${theme}-${state}.png`),fullPage:true});
 }
 try{
  await page.goto('/');await expect(page.getByRole('button',{name:text.share,exact:true})).toBeEnabled();await expect(page.locator('html')).toHaveAttribute('lang',locale==='en'?'en':'zh-CN');
  await expect.poll(()=>page.locator('html').evaluate(element=>element.classList.contains('dark'))).toBe(theme==='dark');await capture('empty');
  const shareBox=await page.getByRole('button',{name:text.share,exact:true}).boundingBox(),openBox=await page.getByRole('button',{name:text.open,exact:true}).boundingBox();expect(Math.abs(shareBox.y-openBox.y)).toBeLessThan(2);
  const chooser=page.waitForEvent('filechooser');await page.getByRole('button',{name:text.share,exact:true}).focus();await page.keyboard.press('Enter');
  await(await chooser).setFiles({name:'local-<script>-文件.bin',mimeType:'application/octet-stream',buffer:Buffer.alloc(786449,11)});
  await expect(page.getByRole('heading',{name:text.confirm,exact:true})).toBeVisible();
  await expect.poll(()=>page.evaluate(()=>!!document.activeElement?.closest('[role=dialog]'))).toBe(true);
  for(let i=0;i<7;i++){await page.keyboard.press(browserName==='webkit'?'Alt+Tab':'Tab');const focus=await page.evaluate(()=>({inside:!!document.activeElement?.closest('[role=dialog]'),tag:document.activeElement?.tagName,text:document.activeElement?.textContent?.slice(0,80)}));if(!focus.inside)console.log(JSON.stringify({tab:i,focus}));expect(focus.inside).toBe(true);}
  await capture('share-confirm');await page.getByRole('button',{name:text.startShare,exact:true}).focus();await page.keyboard.press('Enter');
  const share=page.getByRole('article').first();await expect(share.getByText(text.waiting,{exact:true})).toBeVisible();await expect(share.getByRole('button',{name:text.saveSeed,exact:true})).toBeEnabled();await capture('share-active');
  const menu=share.getByRole('button',{name:locale==='en'?/^Details/:/^详情/});await menu.focus();await page.keyboard.press('Enter');
  await expect(page.getByRole('menuitem',{name:text.pause,exact:true})).toBeVisible();await page.keyboard.press('Home');await expect(page.getByRole('menuitem').first()).toBeFocused();await page.keyboard.press('ArrowDown');await expect(page.getByRole('menuitem',{name:text.pause,exact:true})).toBeFocused();await page.keyboard.press('Enter');await expect(share.getByText(text.paused,{exact:true})).toBeVisible();await capture('share-paused');
  await menu.click();await page.getByRole('menuitem',{name:text.resume,exact:true}).click();await expect(share.getByText(text.waiting,{exact:true})).toBeVisible();
  await menu.click();await page.getByRole('menuitem',{name:text.details,exact:true}).click();await capture('details');await page.keyboard.press('Escape');
  await page.getByRole('button',{name:text.settings,exact:true}).click();await capture('settings');
  await page.getByRole('button',{name:theme==='light'?text.dark:text.light,exact:true}).click();await expect.poll(()=>page.locator('html').evaluate(element=>element.classList.contains('dark'))).toBe(theme!=='dark');
  await page.getByRole('button',{name:text.system,exact:true}).click();await expect.poll(()=>page.locator('html').evaluate(element=>element.classList.contains('dark'))).toBe(theme==='dark');await page.keyboard.press('Escape');
  await page.locator('input[type=file][accept]').setInputFiles({name:'matrix.q2q',mimeType:'application/octet-stream',buffer:Buffer.from(seed)});await page.getByRole('button',{name:text.startDownload,exact:true}).click();
  const download=page.getByRole('article').last();
  await expect(download).toBeVisible();await download.getByRole('button',{name:text.pause,exact:true}).focus();await page.keyboard.press('Enter');await expect(download.getByText(text.paused,{exact:true})).toBeVisible();await capture('download-paused');
  slow=false;await download.getByRole('button',{name:text.resume,exact:true}).focus();await page.keyboard.press('Enter');await expect(page.getByText(text.completed,{exact:true})).toBeVisible({timeout:20000});await capture('download-complete');
  await expect(page.locator('script[src=x],img[src=x]')).toHaveCount(0);
  // Stop the source and create a visible, bounded no-source failure.
  await menu.click();await page.getByRole('menuitem',{name:text.stop,exact:true}).click();
  const offlineBuilder=new SeedBuilder(1);offlineBuilder.update(new Uint8Array([99]));const offline=offlineBuilder.finish_inline('offline.bin');
  await page.locator('input[type=file][accept]').setInputFiles({name:'offline.q2q',mimeType:'application/octet-stream',buffer:Buffer.from(offline.seed_bytes())});offline.free();await page.getByRole('button',{name:text.startDownload,exact:true}).click();
  await expect(page.getByRole('article',{name:'offline.bin',exact:true}).getByText(text.failed,{exact:true})).toBeVisible({timeout:15000});await capture('error');
  expect(errors).toEqual([]);
 }finally{await page.goto('about:blank').catch(()=>{});await services.close();originServer.closeAllConnections();await new Promise(resolve=>originServer.close(resolve));}
});

test('explicit theme persists while system preference only follows OS in system mode',async({page,context})=>{
 await context.route(/^https:\/\/track\.(?:test\.invalid|q2q\.app)\//,route=>route.fulfill({status:503,json:{code:'isolated-ui-test'}}));
 await page.emulateMedia({colorScheme:'dark'});await page.goto('/');await expect(page.getByRole('button',{name:'分享文件',exact:true})).toBeEnabled();
 const dark=()=>page.locator('html').evaluate(element=>element.classList.contains('dark'));
 await expect.poll(dark).toBe(true);await page.getByRole('button',{name:'设置',exact:true}).focus();await page.keyboard.press('Enter');
 await page.getByRole('button',{name:'浅色',exact:true}).click();await expect.poll(dark).toBe(false);
 await page.keyboard.press('Escape');await page.reload();await expect(page.getByRole('button',{name:'分享文件',exact:true})).toBeEnabled();await expect.poll(dark).toBe(false);
 await page.getByRole('button',{name:'设置',exact:true}).focus();await page.keyboard.press('Enter');await page.getByRole('button',{name:'跟随系统',exact:true}).click();await expect.poll(dark).toBe(true);
 await page.emulateMedia({colorScheme:'light'});await expect.poll(dark).toBe(false);
 await page.keyboard.press('Escape');await expect(page.getByRole('button',{name:'设置',exact:true})).toBeFocused();
});
