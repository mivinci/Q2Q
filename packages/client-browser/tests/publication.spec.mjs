import {test,expect} from '@playwright/test';
import {startServer} from './browser-server.mjs';
import {networkFixture} from './network-fixture.mjs';

test('Web Publication serves a complete file through real Track Punch and WebRTC @publish',async({page,context})=>{
 test.setTimeout(90000);
 const {server,origin}=await startServer();const services=await networkFixture();let receiver;
 try{
  await page.goto(origin);
  const prepared=await page.evaluate(async track=>(await import('/tests/publication.browser.ts')).start(track),services.trackUrl);
  receiver=await context.newPage();await receiver.goto(origin);
  await receiver.evaluate(async({track,seed})=>(await import('/tests/publication.browser.ts')).receive(track,seed),{track:services.trackUrl,seed:prepared.seed});
  await page.evaluate(async()=>(await import('/tests/publication.browser.ts')).pause());
  await page.evaluate(async()=>(await import('/tests/publication.browser.ts')).resume());
  expect(await page.evaluate(async()=>(await import('/tests/publication.browser.ts')).state())).toBe('publishing');
 }finally{
  await page.evaluate(async()=>(await import('/tests/publication.browser.ts')).close()).catch(()=>{});
  await receiver?.close();await services.close();await new Promise(resolve=>server.close(resolve));
 }
});

test('native SDK downloads Web publication with independent file hash @publish',async({page,browserName})=>{
 test.skip(browserName!=='chromium','WebKit/native ICE remains unverified');test.setTimeout(90000);
 const {execFile}=await import('node:child_process');const {promisify}=await import('node:util');
 const execute=promisify(execFile);const {mkdtemp,writeFile,rm}=await import('node:fs/promises');const {tmpdir}=await import('node:os');const {join}=await import('node:path');
 const {fileURLToPath}=await import('node:url');const root=fileURLToPath(new URL('../../../',import.meta.url));
 const compiled=await execute('cargo',['test','--locked','--offline','-p','q2q-core','--test','sdk','--no-run','--message-format=json'],{cwd:root,maxBuffer:8*1024*1024});
 const artifact=compiled.stdout.split('\n').filter(Boolean).map(line=>JSON.parse(line)).find(row=>row.reason==='compiler-artifact'&&row.target.name==='sdk'&&row.executable);
 if(!artifact)throw Error('Native SDK fixture missing');
 const folder=await mkdtemp(join(tmpdir(),'q2q-browser-upload-'));const {server,origin}=await startServer();const services=await networkFixture();
 try{
  await page.goto(origin);const prepared=await page.evaluate(async track=>(await import('/tests/publication.browser.ts')).start(track),services.trackUrl);
  const path=join(folder,'seed.q2q');await writeFile(path,Buffer.from(prepared.seed));
  const result=await execute(artifact.executable,['native_downloads_browser_publication','--ignored','--nocapture'],{cwd:root,env:{...process.env,Q2Q_BROWSER_UPLOAD_TRACK:services.trackUrl,Q2Q_BROWSER_UPLOAD_SEED:path},timeout:50000});
  expect(result.stdout).toContain('Browser publication verified by native SDK');
 }finally{await page.evaluate(async()=>(await import('/tests/publication.browser.ts')).close()).catch(()=>{});await services.close();await new Promise(resolve=>server.close(resolve));await rm(folder,{recursive:true,force:true});}
});

for(const scenario of ['closing','pause-preparation'])test(`publication preparation lifecycle ${scenario} @publish`,async({page})=>{
 const {server,origin}=await startServer();const services=await networkFixture();
 try{await page.goto(origin);await page.evaluate(async({track,scenario})=>{
  const module=await import('/tests/publication.browser.ts');
  if(scenario==='closing')await module.checkLifecycle(track);else await module.checkPauseDuringPreparation(track);
 },{track:services.trackUrl,scenario});}
 finally{await services.close();await new Promise(resolve=>server.close(resolve));}
});

test('publication pause withdraws and resume restores only this content @publish',async({page})=>{
 const {server,origin}=await startServer();const services=await networkFixture();
 try{await page.goto(origin);const prepared=await page.evaluate(async track=>(await import('/tests/publication.browser.ts')).start(track),services.trackUrl);
 const listing=()=>page.evaluate(async({track,id})=>(await import('/tests/publication.browser.ts')).listing(track,id),{track:services.trackUrl,id:prepared.id});
 expect(await listing()).toBe(1);
 await page.evaluate(async()=>(await import('/tests/publication.browser.ts')).pause());expect(await listing()).toBe(0);
 await page.evaluate(async()=>(await import('/tests/publication.browser.ts')).resume());expect(await listing()).toBe(1);
 await page.evaluate(async()=>(await import('/tests/publication.browser.ts')).close());expect(await listing()).toBe(0);
 }finally{await page.evaluate(async()=>(await import('/tests/publication.browser.ts')).close()).catch(()=>{});await services.close();await new Promise(resolve=>server.close(resolve));}
});

test('changed source fails publication and does not serve unverified bytes @publish',async({page,context})=>{
 const {server,origin}=await startServer();const services=await networkFixture();let receiver;
 try{await page.goto(origin);const prepared=await page.evaluate(async track=>(await import('/tests/publication.browser.ts')).start(track,true),services.trackUrl);
 await page.evaluate(async()=>(await import('/tests/publication.browser.ts')).change());receiver=await context.newPage();await receiver.goto(origin);
 const rejected=await receiver.evaluate(async({track,seed})=>{try{await(await import('/tests/publication.browser.ts')).receive(track,seed);return false;}catch{return true;}},{track:services.trackUrl,seed:prepared.seed});
 expect(rejected).toBe(true);expect(await page.evaluate(async()=>(await import('/tests/publication.browser.ts')).state())).toBe('failed');
 }finally{await page.evaluate(async()=>(await import('/tests/publication.browser.ts')).close()).catch(()=>{});await receiver?.close();await services.close();await new Promise(resolve=>server.close(resolve));}
});

test('empty publication and reentrant observer close keep lifecycle idempotent @publish',async({page})=>{
 const {server,origin}=await startServer();const services=await networkFixture();
 try{await page.goto(origin);await page.evaluate(async track=>(await import('/tests/publication.browser.ts')).checkEmptyAndReentrantClose(track),services.trackUrl);}
 finally{await services.close();await new Promise(resolve=>server.close(resolve));}
});

test('browser publication retransmits a dropped data piece with the existing NACK protocol @publish',async({page,context})=>{
 const {server,origin}=await startServer();const services=await networkFixture();let receiver;
 try{await page.goto(origin);const prepared=await page.evaluate(async track=>(await import('/tests/publication.browser.ts')).start(track),services.trackUrl);
 receiver=await context.newPage();await receiver.goto(origin);
 await receiver.evaluate(async({track,seed})=>(await import('/tests/publication.browser.ts')).receive(track,seed,true),{track:services.trackUrl,seed:prepared.seed});
 }finally{await page.evaluate(async()=>(await import('/tests/publication.browser.ts')).close()).catch(()=>{});await receiver?.close();await services.close();await new Promise(resolve=>server.close(resolve));}
});
