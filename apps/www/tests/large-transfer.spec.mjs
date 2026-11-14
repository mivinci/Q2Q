import {test,expect,chromium} from '@playwright/test';
import {mkdtemp,rm,open,readFile,statfs,writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {tmpdir} from 'node:os';
import {createHash} from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {networkFixture} from '../../../packages/client-web/tests/network-fixture.mjs';

// Explicit opt-in: normal CI does not create multi-GiB profiles or test files.
for(const mib of [100,1024])test(`large actual-page publication and P2P download ${mib} MiB @large-transfer`,async({},testInfo)=>{
 test.skip(process.env.TEST_LARGE_TRANSFER!=='1','Explicit local disk and runtime budget required');
 test.skip(testInfo.project.name!=='chromium','Memory sampling and native-file input acceptance use Chromium');
 test.setTimeout(20*60*1000);
 const disk=await statfs(tmpdir());expect(disk.bavail*disk.bsize).toBeGreaterThan(5*1024**3);
 const folder=await mkdtemp(join(tmpdir(),'q2q-www-large-transfer-')),name=`large-${mib}.bin`,size=mib*1024*1024;
 let sender,receiver,services,timer;const rootPids=new Set();let peak=0,samples=0;const phases={};
 const report={mib,size,phases,method:'500ms browser process-tree RSS sum; shared pages may be counted twice and brief peaks missed',saveTarget:'OPFS stand-in for a selected save handle; not OS user-file durability'};
 function sample(){
  if(!rootPids.size)return null;
  const rows=execFileSync('ps',['-axo','pid=,ppid=,rss='],{encoding:'utf8'}).trim().split('\n').map(row=>row.trim().split(/\s+/).map(Number));
  const ids=new Set(rootPids);let changed;
  do{changed=false;for(const[pid,parent]of rows)if(ids.has(parent)&&!ids.has(pid)){ids.add(pid);changed=true;}}while(changed);
  const value=rows.reduce((sum,[pid,,rss])=>sum+(ids.has(pid)?rss*1024:0),0);peak=Math.max(peak,value);samples++;return value;
 }
 const launch={headless:true,acceptDownloads:true,viewport:{width:1280,height:800},args:['--use-mock-keychain'],...(process.env.CHROME_PATH?{executablePath:process.env.CHROME_PATH}:{})};
 const baseURL='http://127.0.0.1:5180';
 async function routeTrack(context){await context.route(/^https:\/\/track\.(?:test\.invalid|q2q\.app)\//,async route=>{
  const url=new URL(route.request().url());const response=await route.fetch({url:services.trackUrl+url.pathname+url.search});await route.fulfill({response});
 });}
 async function phase(name,operation){console.log(`${mib} MiB: ${name}`);const start=Date.now();const value=await operation();phases[name]=Date.now()-start;return value;}
 async function verifyOpfs(page,exportName){
  const digest=createHash('sha256');
  // Bounded 1 MiB CDP copies feed an independent Node streaming SHA-256.
  for(let offset=0;offset<size;offset+=1048576){
   const encoded=await page.evaluate(async({name,offset,length})=>{
    const root=await navigator.storage.getDirectory(),file=await(await root.getFileHandle(name)).getFile();
    const bytes=new Uint8Array(await file.slice(offset,offset+length).arrayBuffer());let binary='';
    for(let i=0;i<bytes.length;i+=8192)binary+=String.fromCharCode(...bytes.subarray(i,i+8192));return btoa(binary);
   },{name:exportName,offset,length:Math.min(1048576,size-offset)});
   digest.update(Buffer.from(encoded,'base64'));
  }
  return digest.digest('hex');
 }
 try{
  const path=join(folder,name),file=await open(path,'wx'),hash=createHash('sha256');
  try{for(let offset=0;offset<size;offset+=1048576){const bytes=Buffer.alloc(Math.min(1048576,size-offset));for(let i=0;i<bytes.length;i++)bytes[i]=((offset+i)*31+Math.floor((offset+i)/262144))%251;await file.write(bytes);hash.update(bytes);}}finally{await file.close();}
  report.expectedSha256=hash.digest('hex');services=await networkFixture();
  sender=await chromium.launchPersistentContext(join(folder,'sender'),{...launch,baseURL});await routeTrack(sender);
  const sending=await sender.newPage();await sending.goto('/');await expect(sending.getByRole('button',{name:'分享文件',exact:true})).toBeEnabled();
  const cdp=await sender.browser().newBrowserCDPSession();const {processInfo}=await cdp.send('SystemInfo.getProcessInfo');rootPids.add(processInfo.find(row=>row.type==='browser').id);await cdp.detach();
  report.senderOnlyBaselineRss=sample();timer=setInterval(sample,500);
  await sending.locator('input[type=file]:not([accept])').setInputFiles(path);await sending.getByRole('button',{name:'开始分享',exact:true}).click();
  const share=sending.getByRole('article',{name:`分享: ${name}`,exact:true});await phase('prepare',()=>expect(share.getByText('等待接收',{exact:true})).toBeVisible({timeout:120000}));
  const seedDownload=sending.waitForEvent('download');await share.getByRole('button',{name:'保存种子',exact:true}).click();const seed=await readFile(await(await seedDownload).path());
  receiver=await chromium.launchPersistentContext(join(folder,'receiver'),{...launch,baseURL});await routeTrack(receiver);
  const receiverCdp=await receiver.browser().newBrowserCDPSession();const receiverInfo=await receiverCdp.send('SystemInfo.getProcessInfo');rootPids.add(receiverInfo.processInfo.find(row=>row.type==='browser').id);await receiverCdp.detach();report.combinedBaselineRss=sample();
  const receiving=await receiver.newPage();await receiving.addInitScript(()=>Object.defineProperty(window,'showSaveFilePicker',{value:undefined,configurable:true}));await receiving.goto('/');
  await expect(receiving.getByRole('button',{name:'下载',exact:true})).toBeEnabled();
  await receiving.locator('input[type=file][accept]').setInputFiles({name:'large.q2q',mimeType:'application/octet-stream',buffer:seed});await receiving.getByRole('button',{name:'开始下载',exact:true}).click();
  const row=receiving.getByRole('article',{name,exact:true});
  const usage=async()=>{const session=await receiver.newCDPSession(receiving);try{return(await session.send('Storage.getUsageAndQuota',{origin:baseURL})).usage;}finally{await session.detach();}};
  await phase('cancelAfterCommittedBlocks',async()=>{
    await expect.poll(()=>receiving.evaluate(()=>Object.entries(localStorage).filter(([key])=>key.startsWith('q2q-web-transfer-v1:')).map(([,value])=>JSON.parse(value)).some(value=>BigInt(value.committed)>=4n*1024n*1024n&&!value.complete)),{timeout:60000}).toBe(true);
    await row.getByRole('button',{name:/详情/}).click();await receiving.getByRole('menuitem',{name:'取消下载',exact:true}).click();
    await expect(row.getByText('已取消',{exact:true})).toBeVisible({timeout:10000});
    report.cancelledCheckpointBytes=await receiving.evaluate(()=>Object.entries(localStorage).filter(([key])=>key.startsWith('q2q-web-transfer-v1:')).map(([,value])=>JSON.parse(value))[0].committed);
    expect(BigInt(report.cancelledCheckpointBytes)).toBeGreaterThan(0n);expect(BigInt(report.cancelledCheckpointBytes)).toBeLessThan(BigInt(size));
    await row.getByRole('button',{name:'重试',exact:true}).click();
  });
  await phase('transfer',()=>expect(row.getByText('待保存',{exact:true})).toBeVisible({timeout:15*60*1000}));report.originAfterDownload=await usage();
  await row.getByRole('button',{name:'保存文件',exact:true}).click();await expect(row.getByText('此浏览器无法流式保存超过 32 MiB 的文件。',{exact:false})).toBeVisible();
  report.boundedBlobFallbackRefused=true;
  const exportName=`large-export-${mib}.bin`;
  await receiving.evaluate(name=>Object.defineProperty(window,'showSaveFilePicker',{value:async()=>{const root=await navigator.storage.getDirectory();return root.getFileHandle(name,{create:true});},configurable:true}),exportName);
  await phase('streamExport',async()=>{await row.getByRole('button',{name:'保存文件',exact:true}).click();await expect(row.getByText('正在保存',{exact:true})).toBeHidden({timeout:180000});});
  expect(await receiving.evaluate(async name=>(await(await(await navigator.storage.getDirectory()).getFileHandle(name)).getFile()).size,exportName)).toBe(size);
  report.originAfterExport=await usage();report.actualSha256=await phase('independentHash',()=>verifyOpfs(receiving,exportName));expect(report.actualSha256).toBe(report.expectedSha256);
  await expect(row.getByText('已保存到文件',{exact:true})).toHaveCount(0);report.opfsNotMisreportedAsUserFile=true;
  await share.getByRole('button',{name:/详情/}).click();await sending.getByRole('menuitem',{name:'停止分享',exact:true}).click();
  await row.getByRole('button',{name:/详情/}).click();await receiving.getByRole('menuitem',{name:'删除暂存',exact:true}).click();await receiving.getByRole('dialog').getByRole('button',{name:'删除暂存',exact:true}).click();await expect(row).toHaveCount(0);
  await receiving.evaluate(async name=>(await navigator.storage.getDirectory()).removeEntry(name),exportName);report.originAfterCleanup=await usage();
  report.combinedPeakProcessTreeRss=peak;report.rssSamples=samples;report.browser=sender.browser().version();
  console.log(JSON.stringify(report));await writeFile(new URL(`../../../.tmp/www-large-transfer-${mib}.json`,import.meta.url),JSON.stringify(report,null,2)+'\n');await testInfo.attach('large-transfer.json',{body:JSON.stringify(report,null,2),contentType:'application/json'});
 }catch(error){report.failed=true;report.error=String(error);console.log(JSON.stringify(report));throw error;}
 finally{clearInterval(timer);await receiver?.close();await sender?.close();await services?.close();await rm(folder,{recursive:true,force:true});}
});
