import { test, expect } from '@playwright/test';
import { startServer } from './browser-server.mjs';
import { p2pFixture } from './p2p-fixture.mjs';

for(const scenario of ['download','pause','reload'])test(`public Track and trusted WSS verified native content: ${scenario} @public`, async ({page,browserName})=>{
  test.skip(process.env.TEST_PUBLIC_P2P!=='1','Explicit production session/index/signalling authorization required');
  test.skip(browserName!=='chromium','WebKit/native ICE is not validated');
  test.setTimeout(300000);
  const trackUrl=process.env.Q2Q_PUBLIC_TRACK_URL;
  if(!trackUrl?.startsWith('https://'))throw Error('Set an explicitly approved HTTPS Q2Q_PUBLIC_TRACK_URL');
  const {server,origin}=await startServer();let services;
  const sockets=[];let contentRequests=0;
  page.on('websocket',socket=>{
    sockets.push(socket.url());const methods=new Map();
    socket.on('framesent',frame=>{try{const value=JSON.parse(frame.payload.toString());if(value.type==='request')methods.set(value.payload.id,value.payload.method);}catch{}});
    socket.on('framereceived',frame=>{try{const value=JSON.parse(frame.payload.toString());if(value.type==='response'&&value.payload.error)console.log(JSON.stringify({punchMethod:methods.get(value.payload.id),errorCode:value.payload.error.code}));}catch{}});
  });
  // No routes, injected frames, certificate overrides or HTTP content fixture.
  page.on('request',request=>{if(request.resourceType()==='fetch'&&!request.url().startsWith(origin)&&!request.url().startsWith(trackUrl))contentRequests++;});
  try {
    // QUIC and WSS peers share a finite join/exit network-prefix budget.
    console.log('Waiting one production quota window before starting');
    await new Promise(resolve=>setTimeout(resolve,65000));
    services=await p2pFixture({trackUrl});
    await page.goto(origin);
    console.log(`Public stage: ${scenario}`);
    if(scenario==='reload'){
      const saved=await page.evaluate(async fixture=>(await import('/tests/p2p.browser.ts')).prepareSparseReload(fixture),{seed:services.externalSeed,trackUrl});
      await page.reload();
      await page.evaluate(async saved=>(await import('/tests/p2p.browser.ts')).checkSparseReload(saved,'none'),saved);
    }else{
      await page.evaluate(async({fixture,pause})=>(await import('/tests/p2p.browser.ts')).download(fixture,pause),
        {fixture:{seed:services.seed,trackUrl,heartbeatMs:30000},pause:scenario==='pause'});
    }
    expect(sockets.length).toBeGreaterThan(0);
    expect(sockets.every(url=>url==='wss://punch-hk-1.q2q.app/')).toBe(true);
    expect(contentRequests).toBe(0);
    await services.finish();
    console.log(JSON.stringify({trackUrl,wss:[...new Set(sockets)],bytes:786449,sha256:'3ba31e034e8297dab9200e0e697a3440048b3d9d113d5460ad0b5cff9c324cd1',scenario,verified:true,httpContentRequests:contentRequests,note:'Both peers local; not cross-NAT or forced TURN acceptance.'}));
  } catch(error) {if(services)console.log(JSON.stringify({nativeDiagnostics:services.diagnostics()}));throw error;} finally {await services?.close();await new Promise(resolve=>server.close(resolve));}
});
