import { test as base, expect } from '@playwright/test';
import { startServer } from './browser-server.mjs';
import { networkFixture } from './network-fixture.mjs';
import { cases } from './network.browser.ts';
const test=base.extend({
  origin:[async({},use)=>{const {server,origin}=await startServer();try{await use(origin);}finally{await new Promise(resolve=>server.close(resolve));}},{scope:'worker'}],
  services:async({},use)=>{const fixture=await networkFixture();try{await use(fixture);}finally{await fixture.close();}}
});
for(const name of Object.keys(cases))test(`${name} @network`,async({page,origin,services})=>{
  await page.goto(origin);
  await page.evaluate(async({name,fixture})=>(await import('/tests/network.browser.ts')).cases[name](fixture),{name,fixture:{trackUrl:services.trackUrl,punch:services.punch}});
});
for (const mode of ['reorder','quic-only','legacy','duplicate','public-ws','fragment']) {
  test(`Track assignment endpoint validation: ${mode} @network`,async({page,origin,services})=>{
    let sockets=0;
    page.on('websocket',()=>sockets++);
    await page.route(`${services.trackUrl}/peers/*/files`,async route=>{
      const request=route.request().postDataJSON();
      expect(request.punch_schemes).toEqual(['wss','ws']);
      expect(request).not.toHaveProperty('punch_transport');
      const response=await route.fetch();const body=await response.json();
      const ws=services.punch.endpoints.find(url=>url.startsWith('ws:'));
      if(mode==='reorder')body.punch.endpoints=[...body.punch.endpoints].reverse();
      if(mode==='quic-only')body.punch.endpoints=body.punch.endpoints.filter(url=>url.startsWith('quic:'));
      if(mode==='legacy'){delete body.punch.endpoints;body.punch.websocket_url=ws;}
      if(mode==='duplicate')body.punch.endpoints=[ws,ws];
      if(mode==='public-ws')body.punch.endpoints=['ws://public.invalid/ws'];
      if(mode==='fragment')body.punch.endpoints=[`${ws}#secret`];
      await route.fulfill({response,json:body});
    });
    await page.goto(origin);
    const result=await page.evaluate(async({trackUrl})=>{
      const {createClient}=await import('/src/index.ts');const client=createClient({trackUrl});
      try{const network=await client.connectNetwork();return{state:network.snapshot().state,snapshot:JSON.stringify(network.snapshot())};}
      catch(error){return{code:error.code};}finally{await client.close();}
    },{trackUrl:services.trackUrl});
    if(mode==='reorder'){
      expect(result.state).toBe('online');expect(sockets).toBe(1);
      expect(result.snapshot).not.toMatch(/endpoints|ws:\/\/|session_key|binding_key|credential/);
    }else{expect(result.code).toBe(mode==='quic-only'?'PUNCH_UNAVAILABLE':'NETWORK_PROTOCOL_ERROR');expect(sockets).toBe(0);}
  });
}

test('WSS is preferred over loopback WS without deriving an address from QUIC @network',async({page,origin,services})=>{
  const publicEndpoint='wss://proxy.invalid:8443/independent/signal?region=hk';
  const opened=[];page.on('websocket',socket=>opened.push(socket.url()));
  await page.route(`${services.trackUrl}/peers/*/files`,async route=>{
    const response=await route.fetch(),body=await response.json();
    body.punch.endpoints=[...body.punch.endpoints,publicEndpoint];await route.fulfill({response,json:body});
  });
  // Host WebSocket selection only; this mock does not validate public TLS or Punch authentication.
  await page.routeWebSocket(publicEndpoint,socket=>{
    opened.push(publicEndpoint);
    socket.onMessage(data=>{
    const request=JSON.parse(data.toString()).payload;
    const result=request.method==='punch.join'?{binding_id:'mock-binding',binding_key:Buffer.alloc(32,8).toString('base64'),expires_at:Math.floor(Date.now()/1000)+300}:{};
    socket.send(Buffer.from(JSON.stringify({type:'response',payload:{v:1,id:request.id,result}})));
    });
  });
  await page.goto(origin);
  await page.evaluate(async trackUrl=>{
    const {createClient}=await import('/src/index.ts');const client=createClient({trackUrl});
    try{const network=await client.connectNetwork();if(network.snapshot().state!=='online')throw Error('not online');}
    finally{await client.close();}
  },services.trackUrl);
  expect(opened).toEqual([publicEndpoint]);
});

for(const mode of ['full-list','quic-only','old-field'])test(`Peer candidate endpoint filtering: ${mode} @network`,async({page,origin,services})=>{
  await page.goto(origin);
  const peer=await page.evaluate(async()=>{
    const {controlIdentity}=await import('/src/control-identity.ts');const own=await controlIdentity();
    try{return own.info;}finally{own.close();}
  });
  await page.route(`${services.trackUrl}/files/*/peers?*`,async route=>{
    const punch={...services.punch,endpoints:[...services.punch.endpoints].reverse()};
    if(mode==='quic-only')punch.endpoints=punch.endpoints.filter(url=>url.startsWith('quic:'));
    if(mode==='old-field')punch.websocket_url='wss://old.invalid/ws';
    await route.fulfill({json:{peers:[{...peer,punch,complete:true}],next_cursor:null}});
  });
  const result=await page.evaluate(async trackUrl=>{
    const {createClient}=await import('/src/index.ts');const {peerNetwork}=await import('/src/network.ts');
    const client=createClient({trackUrl});
    try{const session=await client.connectNetwork();const peers=await peerNetwork(session).query(`sha256:${'01'.repeat(32)}`,new AbortController().signal);return{peers};}
    catch(error){return{code:error.code};}finally{await client.close();}
  },services.trackUrl);
  if(mode==='old-field')expect(result.code).toBe('NETWORK_PROTOCOL_ERROR');
  else if(mode==='quic-only')expect(result.peers).toEqual([]);
  else{expect(result.peers).toHaveLength(1);expect(result.peers[0].websocketUrl).toBe(services.punch.endpoints.find(url=>url.startsWith('ws:')));}
});

test('server disconnect becomes offline and closes the session @network',async({page,origin,services})=>{
  await page.goto(origin);
  await page.evaluate(async fixture=>(await import('/tests/network.browser.ts')).prepareDisconnect(fixture),{trackUrl:services.trackUrl,punch:services.punch});
  await services.stopPunch();
  await page.evaluate(async()=>(await import('/tests/network.browser.ts')).checkDisconnect());
});


test('automatic heartbeat runs and close removes its future timers @network',async({page,origin,services})=>{
  await page.clock.install({time:new Date()});await page.goto(origin);
  let requests=0;page.on('request',request=>{if(request.url().endsWith('/heartbeat'))requests++;});
  await page.evaluate(async fixture=>(await import('/tests/network.browser.ts')).prepareDisconnect(fixture),{trackUrl:services.trackUrl,punch:services.punch});
  await page.clock.fastForward(30001);
  await expect.poll(()=>requests).toBeGreaterThan(0);
  await page.evaluate(async()=>(await import('/tests/network.browser.ts')).checkAutomaticHeartbeat());
  await page.evaluate(async()=>(await import('/tests/network.browser.ts')).closeHeld());
  const count=requests;await page.clock.fastForward(120000);expect(requests).toBe(count);
});

test('RPC timeout, abort, late replies and bounded pending calls @network',async({page,origin})=>{
  await page.routeWebSocket('ws://127.0.0.1:19999/control',socket=>{
    socket.onMessage(data=>{
      const request=JSON.parse(data.toString()).payload;
      if(request.params.mode==='silent')return;
      const reply=()=>socket.send(Buffer.from(JSON.stringify({type:'response',payload:{v:1,id:request.id,result:{ok:true}}})));
      if(request.params.mode==='late')setTimeout(reply,20);else reply();
    });
  });
  await page.goto(origin);
  await page.evaluate(async()=>(await import('/tests/network.browser.ts')).checkRpcFaults('ws://127.0.0.1:19999/control'));
});
for(const mode of ['text','oversize','invalid'])test(`invalid ${mode} frame terminates pending RPC @network`,async({page,origin})=>{
  await page.routeWebSocket('ws://127.0.0.1:19999/control',socket=>{
    socket.onMessage(()=>socket.send(mode==='text'?'{}':mode==='oversize'?Buffer.alloc(65537):Buffer.from('{}')));
  });
  await page.goto(origin);
  await page.evaluate(async()=>(await import('/tests/network.browser.ts')).checkMalformedFrame('ws://127.0.0.1:19999/control'));
});
