import {test,expect} from '@playwright/test';
import {startServer} from './browser-server.mjs';
import {networkFixture} from './network-fixture.mjs';
import {p2pFixture} from './p2p-fixture.mjs';

for(const mode of ['memory','opfs','metadata-fail','pause','close-sharing','sparse','corrupt-committed','resume'])test(`partial publication committed readback and lifecycle: ${mode} @partial`,async({page,context,browserName})=>{
 test.skip(mode==='sparse'&&browserName==='webkit','WebKit/native ICE remains unverified');
 test.skip(mode==='opfs'&&browserName==='webkit','Real OPFS concurrency tested on Chromium; private WebKit target is unavailable');
 test.setTimeout(60000);
 let release;const gate=new Promise(resolve=>{release=resolve;});
 const {server,origin}=await startServer({handleRequest:async(request,response)=>{
  if(!request.url.startsWith('/partial-content'))return false;
  const match=/bytes=(\d+)-(\d+)/.exec(request.headers.range??'');if(!match){response.writeHead(416).end();return true;}
  const start=Number(match[1]),end=Number(match[2]),size=3*262144+17;
  if(start>0)await gate;
  const bytes=Buffer.alloc(end-start+1);for(let i=start;i<=end;i++)bytes[i-start]=(i*31+Math.floor(i/262144))%251;
  response.writeHead(206,{'Content-Length':bytes.length,'Content-Range':`bytes ${start}-${end}/${size}`}).end(bytes);return true;
 }});
 const services=mode==='sparse'?await p2pFixture():await networkFixture();let peer;
 try{
  await page.goto(origin);const value=await page.evaluate(async({track,mode,seed})=>(await import('/tests/partial.browser.ts')).start(track,mode,seed),{track:services.trackUrl,mode,seed:services.externalSeed});
  await expect.poll(()=>page.evaluate(async()=>(await import('/tests/partial.browser.ts')).waiting())).toBe(true);
  peer=await context.newPage();await peer.goto(origin);
  await peer.evaluate(async({track,id})=>(await import('/tests/partial.browser.ts')).probe(track,id),{track:services.trackUrl,id:value.id});
  await page.evaluate(async()=>(await import('/tests/partial.browser.ts')).commit());
  if(mode==='metadata-fail'){
    await page.evaluate(async()=>(await import('/tests/partial.browser.ts')).rejectedCommit());return;
  }
  if(mode==='corrupt-committed'){
    await page.evaluate(async()=>(await import('/tests/partial.browser.ts')).corrupt());
    expect(await peer.evaluate(async seed=>(await import('/tests/partial.browser.ts')).consumeFailure(seed),value.seed)).toBe(true);
    await expect.poll(()=>page.evaluate(async()=>(await import('/tests/partial.browser.ts')).sharingFailed())).toBe('failed');return;
  }
  await peer.evaluate(async seed=>(await import('/tests/partial.browser.ts')).getCommitted(seed),value.seed);
  await peer.evaluate(async()=>(await import('/tests/partial.browser.ts')).closeProbe());
  if(mode==='resume'){
    await page.evaluate(async()=>(await import('/tests/partial.browser.ts')).restartShared());
    await peer.evaluate(async({track,id})=>(await import('/tests/partial.browser.ts')).probe(track,id,true),{track:services.trackUrl,id:value.id});
    await peer.evaluate(async seed=>(await import('/tests/partial.browser.ts')).getCommitted(seed,true),value.seed);
    await peer.evaluate(async()=>(await import('/tests/partial.browser.ts')).closeProbe());
  }
  if(mode==='pause'){await page.evaluate(async()=>(await import('/tests/partial.browser.ts')).pause());return;}
  if(mode==='close-sharing')await page.evaluate(async()=>(await import('/tests/partial.browser.ts')).stopSharing());
  release();await page.evaluate(async()=>(await import('/tests/partial.browser.ts')).finish());if(mode==='sparse')await services.finish();
 }finally{release();await peer?.evaluate(async()=>(await import('/tests/partial.browser.ts')).closeProbe()).catch(()=>{});await page.evaluate(async()=>(await import('/tests/partial.browser.ts')).close()).catch(()=>{});await peer?.close();await services.close();await new Promise(resolve=>server.close(resolve));}
});
