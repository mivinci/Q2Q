import { peerNetwork } from '../src/network.ts';
import { createClient, Q2QError } from '../src/index.ts';
import { controlIdentity, decodeKey } from '../src/control-identity.ts';
import { PunchSocket } from '../src/punch-socket.ts';
import type { SignalMessage } from '../src/punch-socket.ts';
function assert(value:unknown,message='assertion failed'):asserts value {if(!value)throw Error(message);}
async function rejects(work:Promise<unknown>,code:string){try{await work;}catch(error){assert(error instanceof Q2QError&&error.code===code,`expected ${code}, got ${String(error)}`);return;}throw Error(`expected ${code}`);}
export type Fixture={trackUrl:string;punch:{punch_id:string;endpoints:string[]}};
export const cases:Record<string,(fixture:Fixture)=>Promise<void>>={
  'network close drains queued authenticated queries and rejects later operations':async fixture=>{
    const client=createClient({trackUrl:fixture.trackUrl});
    try{
      const session=await client.connectNetwork(),access=peerNetwork(session),signal=new AbortController().signal;
      const requests=Array.from({length:3},()=>access.query(`sha256:${'01'.repeat(32)}`,signal).then(()=>null,error=>error));
      await Promise.resolve();await session.close();
      for(const result of await Promise.all(requests))if(result!==null)assert(result instanceof Q2QError&&['TASK_CANCELLED','NETWORK_CLOSED'].includes(result.code));
      await rejects(access.query(`sha256:${'01'.repeat(32)}`,signal),'NETWORK_CLOSED');
    }finally{await client.close();}
  },
  'Client authenticates real Track and Punch and coalesces heartbeat':async fixture=>{
    const client=createClient({trackUrl:fixture.trackUrl});
    try {
      const first=client.connectNetwork();assert(first===client.connectNetwork());
      const session=await first;assert(session.snapshot().state==='online');
      assert(session.snapshot().punchId===fixture.punch.punch_id);
      const json=JSON.stringify(session.snapshot());
      for(const secret of ['session_key','binding_key','credential','ws://','http://'])assert(!json.includes(secret));
      session.subscribe(()=>{throw Error('observer failure');});
      const a=session.heartbeat(),b=session.heartbeat();assert(a===b);await a;
      assert(session.snapshot().state==='online');
      await session.close();assert(session.snapshot().state==='closed');
      await rejects(session.heartbeat(),'NETWORK_CLOSED');
    }finally{await client.close();}
  },
  'two Client identities are isolated and reconnect keeps only its own identity':async fixture=>{
    const a=createClient({trackUrl:fixture.trackUrl}),b=createClient({trackUrl:fixture.trackUrl});
    try {
      const [one,two]=await Promise.all([a.connectNetwork(),b.connectNetwork()]);
      assert(one.snapshot().peerId!==two.snapshot().peerId);
      const peer=one.snapshot().peerId;await one.close();await two.heartbeat();
      const replacement=await a.connectNetwork();assert(replacement!==one&&replacement.snapshot().peerId===peer);
      await replacement.heartbeat();await a.close();await two.heartbeat();
    }finally{await a.close();await b.close();}
  },
  'close during setup cancels network work and cannot reopen Client':async fixture=>{
    const client=createClient({trackUrl:fixture.trackUrl});
    const connecting=client.connectNetwork();const outcome=connecting.then(()=>null,error=>error);
    const closed=client.close();assert(closed===client.close());await closed;
    const result=await outcome;assert(result instanceof Q2QError&&['CLIENT_CLOSED','TASK_CANCELLED'].includes(result.code));
    try{await client.connectNetwork();throw Error('expected closed client');}catch(error){assert(error instanceof Q2QError&&error.code==='CLIENT_CLOSED');}
  },
  'typed binary signalling completes consent, offer, answer and candidate on native Punch':async fixture=>{
    const a=await controlIdentity(),b=await controlIdentity();
    const signal=new AbortController();const mailA:SignalMessage[]=[],mailB:SignalMessage[]=[];
    const failures:Q2QError[]=[];
    const one=await PunchSocket.open({url:fixture.punch.endpoints.find(url=>url.startsWith('ws:'))!,identity:a,signal:signal.signal,onSignal:value=>{mailA.push(value);},onFailure:error=>failures.push(error)});
    const two=await PunchSocket.open({url:fixture.punch.endpoints.find(url=>url.startsWith('ws:'))!,identity:b,signal:signal.signal,onSignal:value=>{mailB.push(value);},onFailure:error=>failures.push(error)});
    async function next(mail:SignalMessage[],kind:SignalMessage['type']){
      const until=performance.now()+5000;
      while(performance.now()<until){const index=mail.findIndex(v=>v.type===kind);if(index>=0)return mail.splice(index,1)[0]!;await new Promise(resolve=>setTimeout(resolve,10));}
      throw Error(`missing ${kind}`);
    }
    try {
      const joinedA=await one.rpc('punch.join',a.join(fixture.punch.punch_id),signal.signal) as {binding_key:string;binding_id:string};
      const joinedB=await two.rpc('punch.join',b.join(fixture.punch.punch_id),signal.signal) as {binding_key:string;binding_id:string};
      const id=crypto.randomUUID();const hash=new Uint8Array(32).fill(9);
      const connect=a.connectionTicket(fixture.punch.punch_id,b.info.peer_id,hash,id);
      await one.rpc('punch.signal',JSON.stringify({type:'connect',connection_id:id,connect_token:connect}),signal.signal);
      assert((await next(mailB,'connect')).connection_id===id);
      await two.rpc('punch.signal',JSON.stringify({type:'accept',connection_id:id}),signal.signal);
      await next(mailA,'accept');
      const offered=await one.rpc('punch.signal',JSON.stringify({type:'offer',connection_id:id,connect_token:connect,sdp:'test-offer'}),signal.signal) as {signal_key?:string};assert(typeof offered.signal_key==='string');
      assert((await next(mailB,'offer')).sdp==='test-offer');
      await two.rpc('punch.signal',JSON.stringify({type:'answer',connection_id:id,sdp:'test-answer'}),signal.signal);
      assert((await next(mailA,'answer')).sdp==='test-answer');
      await one.rpc('punch.signal',JSON.stringify({type:'candidate',connection_id:id,candidate:'test-candidate'}),signal.signal);
      assert((await next(mailB,'candidate')).candidate==='test-candidate');
      await one.rpc('punch.signal',JSON.stringify({type:'cancel',connection_id:id}),signal.signal);
      await next(mailB,'cancel');
      await one.rpc('punch.exit',a.bindingAuth(decodeKey(joinedA.binding_key),joinedA.binding_id,1n,true),signal.signal);
      await two.rpc('punch.exit',b.bindingAuth(decodeKey(joinedB.binding_key),joinedB.binding_id,1n,true),signal.signal);
      assert(failures.length===0);
    }finally{signal.abort();await one.close();await two.close();a.close();b.close();}
  }
};
let held:ReturnType<typeof createClient>|undefined;
let live:Awaited<ReturnType<ReturnType<typeof createClient>['connectNetwork']>>|undefined;
export async function prepareDisconnect(fixture:Fixture){held=createClient({trackUrl:fixture.trackUrl});live=await held.connectNetwork();return live.snapshot().peerId;}
export async function checkDisconnect(){
  const until=performance.now()+5000;
  while(live?.snapshot().state!=='offline'&&performance.now()<until)await new Promise(resolve=>setTimeout(resolve,20));
  assert(live?.snapshot().state==='offline');await rejects(live.heartbeat(),'NETWORK_CLOSED');await held!.close();
}

export async function checkAutomaticHeartbeat(){await live!.heartbeat();assert(live!.snapshot().state==='online');}
export async function closeHeld(){await held!.close();assert(live!.snapshot().state==='closed');}

/** Fault-only host tests complement the real Track/native Punch integration. */
export async function checkRpcFaults(url:string){
  const identity=await controlIdentity();const signal=new AbortController();const failures:Q2QError[]=[];
  const socket=await PunchSocket.open({url,identity,signal:signal.signal,onSignal:()=>{},onFailure:error=>failures.push(error)});
  try{
    const pending=Array.from({length:32},()=>rejects(socket.rpc('punch.join','{"mode":"silent"}',signal.signal,50),'CONTROL_TIMEOUT'));
    await rejects(socket.rpc('punch.join','{}',signal.signal),'RESOURCE_LIMIT');await Promise.all(pending);
    const abort=new AbortController();const cancelled=rejects(socket.rpc('punch.join','{"mode":"late"}',abort.signal),'TASK_CANCELLED');abort.abort();await cancelled;
    await new Promise(resolve=>setTimeout(resolve,100));
    await socket.rpc('punch.join','{}',signal.signal);assert(failures.length===0);
    await rejects(socket.rpc('punch.join',JSON.stringify({large:'x'.repeat(65536)}),signal.signal),'RESOURCE_LIMIT');
    await socket.close();await rejects(socket.rpc('punch.join','{}',signal.signal),'NETWORK_CLOSED');
  }finally{await socket.close();identity.close();}
}
export async function checkMalformedFrame(url:string){
  const identity=await controlIdentity();const signal=new AbortController();let failure:Q2QError|undefined;
  const socket=await PunchSocket.open({url,identity,signal:signal.signal,onSignal:()=>{},onFailure:error=>{failure=error;}});
  try{
    await rejects(socket.rpc('punch.join','{}',signal.signal),'NETWORK_PROTOCOL_ERROR');
    assert(failure?.code==='NETWORK_PROTOCOL_ERROR');
    await rejects(socket.rpc('punch.join','{}',signal.signal),'NETWORK_CLOSED');
  }finally{await socket.close();identity.close();}
}
