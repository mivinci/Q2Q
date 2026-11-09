import { test as base } from '@playwright/test';
import { startServer } from './browser-server.mjs';
import { createHash } from 'node:crypto';
import { p2pFixture } from './p2p-fixture.mjs';
const test=base.extend({
  origin:[async({},use)=>{const {server,origin}=await startServer({handleRequest:async(request,response)=>{
    const pathname=new URL(request.url,'http://localhost').pathname;
    if(pathname==='/p2p-proof'){
      const leaves=[];const size=3*262144+17;
      for(let start=0;start<size;start+=262144){const block=Buffer.alloc(Math.min(262144,size-start));for(let i=0;i<block.length;i++)block[i]=((start+i)*31+Math.floor((start+i)/262144))%251;leaves.push(createHash('sha256').update(Buffer.from([0])).update(block).digest());}
      const proof=Buffer.concat(leaves);response.writeHead(200,{'Content-Length':proof.length}).end(proof);return true;
    }
    if(pathname!=='/p2p-content')return false;
    const range=/^bytes=(\d+)-(\d+)$/.exec(request.headers.range??'');if(!range){response.writeHead(416).end();return true;}
    const start=Number(range[1]),end=Number(range[2]),size=3*262144+17;
    if(start>end||end>=size){response.writeHead(416).end();return true;}
    const bytes=Buffer.alloc(end-start+1);for(let i=start;i<=end;i++)bytes[i-start]=(i*31+Math.floor(i/262144))%251;
    response.writeHead(206,{'Content-Length':bytes.length,'Content-Range':`bytes ${start}-${end}/${size}`}).end(bytes);return true;
  }});try{await use(origin);}finally{await new Promise(resolve=>server.close(resolve));}},{scope:'worker'}],
  services:async({browserName},use)=>{if(browserName==='webkit'&&process.env.TEST_NATIVE_WEBKIT!=='1'){await use(null);return;}const fixture=await p2pFixture();try{await use(fixture);}finally{await fixture.close();}}
});
for(const pause of [false,true])test(`public Client downloads native publication via Track and Punch, pause=${pause} @p2p`,async({page,origin,services,browserName})=>{
  test.skip(browserName==='webkit'&&process.env.TEST_NATIVE_WEBKIT!=='1','WebKit/native ICE remains unverified');
  await page.goto(origin);
  await page.evaluate(async({fixture,pause})=>(await import('/tests/p2p.browser.ts')).download(fixture,pause),{fixture:{seed:services.seed,trackUrl:services.trackUrl},pause});
  await services.finish();
});

for(const fault of ['drop','corrupt','fallback','cancel','external','parallel','proof','sparse','sparse-corrupt','sparse-proof-corrupt','sparse-memory'])test(`P2P ${fault} preserves verification and cleanup @p2p`,async({page,origin,services,browserName})=>{
  test.skip(browserName==='webkit'&&process.env.TEST_NATIVE_WEBKIT!=='1','WebKit/native ICE remains unverified');
  let http=0;page.on('request',request=>{if(request.url().endsWith('/p2p-content'))http++;});
  await page.goto(origin);
  await page.evaluate(async({fixture,fault})=>(await import('/tests/p2p.browser.ts')).download(fixture,false,fault),{fixture:{seed:['external','sparse','sparse-corrupt','sparse-proof-corrupt','sparse-memory'].includes(fault)?services.externalSeed:services.seed,trackUrl:services.trackUrl},fault});
  if(fault==='fallback'&&http===0)throw Error('HTTP fallback was not exercised');
  await services.finish();
});

for(const tamper of ['none','file','proof','missing'])test(`sparse peer checkpoint survives real reload, tamper=${tamper} @p2p`,async({page,origin,services,browserName})=>{
  test.skip(browserName==='webkit'&&process.env.TEST_NATIVE_WEBKIT!=='1','WebKit/native ICE remains unverified');
  await page.goto(origin);
  const saved=await page.evaluate(async fixture=>(await import('/tests/p2p.browser.ts')).prepareSparseReload(fixture),{seed:services.externalSeed,trackUrl:services.trackUrl});
  await page.reload();
  await page.evaluate(async({saved,tamper})=>(await import('/tests/p2p.browser.ts')).checkSparseReload(saved,tamper),{saved,tamper});
  await services.finish();
});
