import { test as base } from '@playwright/test';
import { startServer } from './browser-server.mjs';
import { createHash } from 'node:crypto';
import { p2pFixture } from './p2p-fixture.mjs';
import { networkFixture } from './network-fixture.mjs';
const test=base.extend({
  origin:[async({},use)=>{const {server,origin}=await startServer();try{await use(origin);}finally{await new Promise(resolve=>server.close(resolve));}},{scope:'worker'}],
  services:async({browserName},use)=>{if(browserName==='webkit'&&process.env.TEST_NATIVE_WEBKIT!=='1'){await use(null);return;}const fixture=await p2pFixture();try{await use(fixture);}finally{await fixture.close();}},
  local:[async({},use)=>{const fixture=await networkFixture();try{await use(fixture);}finally{await fixture.close();}},{scope:'worker'}]
});

// The native bridge publishes 3*262144+17 bytes of (i*31 + i/262144) % 251.
const nativeContent=Buffer.alloc(3*262144+17);
for(let i=0;i<nativeContent.length;i++)nativeContent[i]=(i*31+Math.floor(i/262144))%251;
const nativeHash=createHash('sha256').update(nativeContent).digest('hex');

test('magnet resolve, download and verified hash from native publication @p2p @magnet',async({page,origin,services,browserName})=>{
  test.skip(browserName==='webkit'&&process.env.TEST_NATIVE_WEBKIT!=='1','WebKit/native ICE remains unverified');
  await page.goto(origin);
  const fixture={seed:services.seed,trackUrl:services.trackUrl};
  const result=await page.evaluate(async fixture=>(await import('/tests/magnet.browser.ts')).resolveAndDownload(fixture),fixture);
  test.expect(result.hash).toBe(nativeHash);
  // Stop the publisher; a verified cache hit must not need any online peer.
  const link=await page.evaluate(async fixture=>(await import('/tests/magnet.browser.ts')).linkFromSeed(fixture),fixture);
  await services.finish();
  const cached=await page.evaluate(async({trackUrl,link})=>(await import('/tests/magnet.browser.ts')).resolveFromCache(trackUrl,link),{trackUrl:services.trackUrl,link:link.link});
  test.expect(cached).toBe(result.infoHash);
});

test('magnet parse failures stay local and typed @p2p @magnet',async({page,origin,services,browserName})=>{
  test.skip(browserName==='webkit'&&process.env.TEST_NATIVE_WEBKIT!=='1','WebKit/native ICE remains unverified');
  await page.goto(origin);
  await page.evaluate(async trackUrl=>(await import('/tests/magnet.browser.ts')).parseFailures(trackUrl),services.trackUrl);
  await services.finish();
});

test('magnet resolve failures: no track, unknown hash, cancellation @p2p @magnet',async({page,origin,services,browserName})=>{
  test.skip(browserName==='webkit'&&process.env.TEST_NATIVE_WEBKIT!=='1','WebKit/native ICE remains unverified');
  await page.goto(origin);
  await page.evaluate(async trackUrl=>(await import('/tests/magnet.browser.ts')).resolveFailures(trackUrl),services.trackUrl);
  await services.finish();
});

test('corrupt cache record degrades to rediscovery and heals @p2p @magnet',async({page,origin,services,browserName})=>{
  test.skip(browserName==='webkit'&&process.env.TEST_NATIVE_WEBKIT!=='1','WebKit/native ICE remains unverified');
  await page.goto(origin);
  await page.evaluate(async fixture=>(await import('/tests/magnet.browser.ts')).corruptCache(fixture),{seed:services.seed,trackUrl:services.trackUrl});
  await services.finish();
});

test('peer with mismatching metadata is rejected @p2p @magnet',async({page,origin,services,browserName})=>{
  test.skip(browserName==='webkit'&&process.env.TEST_NATIVE_WEBKIT!=='1','WebKit/native ICE remains unverified');
  await page.goto(origin);
  await page.evaluate(async fixture=>(await import('/tests/magnet.browser.ts')).badMetadataPeer(fixture),{seed:services.seed,trackUrl:services.trackUrl});
  await services.finish();
});

test('web publisher serves metadata end to end @p2p @magnet',async({page,context,origin,local})=>{
  test.setTimeout(90000);
  await page.goto(origin);
  const content=new Uint8Array(3*262144+17);
  for(let i=0;i<content.length;i++)content[i]=(i*37+Math.floor(i/262144))%251;
  const nodeHash=createHash('sha256').update(content).digest('hex');
  const prepared=await page.evaluate(async({trackUrl,content})=>(await import('/tests/magnet.browser.ts')).webPublishForMagnet(trackUrl,content),{trackUrl:local.trackUrl,content});
  const receiver=await context.newPage();
  try{
    await receiver.goto(origin);
    // Two RTCPeerConnections in one page never complete ICE; the receiver
    // must be a second page even on the same origin.
    await receiver.evaluate(async({trackUrl,link,expected})=>(await import('/tests/magnet.browser.ts')).webResolveAndDownload(trackUrl,link,expected),
      {trackUrl:local.trackUrl,link:prepared.link,expected:{infoId:prepared.infoId,size:prepared.size,hash:nodeHash}});
  }finally{
    await receiver.close();
    await page.evaluate(async()=>(await import('/tests/magnet.browser.ts')).webPublishClose()).catch(()=>{});
  }
});
