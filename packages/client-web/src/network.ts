import type { UploadHandler, UploadContext } from './peer-upload.ts';
import { punchAssignmentSchema } from '@q2q/punch-discovery';
import { Q2QError } from './error.ts';
import { endpoint, decodeKey, now } from './control-identity.ts';
import type { ControlIdentity } from './control-identity.ts';
import { PunchSocket } from './punch-socket.ts';
import { PermitPool } from './http.ts';
import { dialPeer } from './peer-dial.ts';
import type { PeerCandidate, PeerConnection } from './peer-dial.ts';
import type { Failure } from './client-types.ts';

export interface NetworkSnapshot { readonly state:'connecting'|'online'|'offline'|'closed';readonly peerId:string;readonly punchId:string|null;readonly error:Failure|null }
export interface NetworkSession {
  snapshot():Readonly<NetworkSnapshot>;
  subscribe(listener:(snapshot:Readonly<NetworkSnapshot>)=>void):()=>void;
  heartbeat():Promise<void>;
  close():Promise<void>;
}
export interface PeerNetwork {
  publish(hash:string,handler:UploadHandler,complete?:boolean):Promise<()=>Promise<void>>;
  query(hash:string,signal:AbortSignal):Promise<PeerCandidate[]>;
  dial(target:PeerCandidate,hash:Uint8Array,signal:AbortSignal):Promise<PeerConnection>;
}
const peers=new WeakMap<NetworkSession,PeerNetwork>();
export function peerNetwork(session:NetworkSession):PeerNetwork {const value=peers.get(session);if(!value)throw new Q2QError('NETWORK_CLOSED');return value;}
const integer=(value:unknown,min:number,max:number):number=>{
  if(typeof value!=='number'||!Number.isSafeInteger(value)||value<min||value>max)throw new Q2QError('NETWORK_PROTOCOL_ERROR');
  return value;
};
const expiry=(value:unknown)=>integer(value,Number(now())+1,Number(now())+86400);
/** Bounded, credential-isolated control requests; never follow redirects. */
async function jsonRequest(url:URL,body:string|undefined,method:'PUT'|'POST'|'GET',signal:AbortSignal,timeout:number):Promise<Record<string,unknown>> {
  const controller=new AbortController();const abort=()=>controller.abort();signal.addEventListener('abort',abort,{once:true});if(signal.aborted)abort();
  const timer=setTimeout(abort,timeout);let reader:ReadableStreamDefaultReader<Uint8Array>|undefined;
  try {
    const response=await fetch(url,{method,headers:{'Content-Type':'application/json'},body,signal:controller.signal,mode:'cors',credentials:'omit',cache:'no-store',redirect:'error',referrerPolicy:'no-referrer'});
    if(!response.ok)await response.body?.cancel().catch(()=>{});
    if(response.status===401||response.status===403)throw new Q2QError('AUTH_REQUIRED');
    if(!response.ok)throw new Q2QError('NETWORK_UNAVAILABLE',response.status>=500||response.status===429);
    if(!response.body)throw new Q2QError('NETWORK_PROTOCOL_ERROR');
    reader=response.body.getReader();const chunks:Uint8Array[]=[];let length=0;
    for(;;){const next=await reader.read();if(next.done)break;length+=next.value.length;if(length>256*1024)throw new Q2QError('RESOURCE_LIMIT');chunks.push(next.value);}
    const bytes=new Uint8Array(length);let offset=0;for(const chunk of chunks){bytes.set(chunk,offset);offset+=chunk.length;}
    let value:unknown;try{value=JSON.parse(new TextDecoder('utf-8',{fatal:true}).decode(bytes));}catch{throw new Q2QError('NETWORK_PROTOCOL_ERROR');}
    if(!value||typeof value!=='object'||Array.isArray(value))throw new Q2QError('NETWORK_PROTOCOL_ERROR');
    return value as Record<string,unknown>;
  }catch(error){
    if(signal.aborted)throw new Q2QError('TASK_CANCELLED');
    if(controller.signal.aborted)throw new Q2QError('CONTROL_TIMEOUT',true);
    if(error instanceof Q2QError)throw error;
    throw new Q2QError('NETWORK_UNAVAILABLE',true);
  }finally{clearTimeout(timer);signal.removeEventListener('abort',abort);await reader?.cancel().catch(()=>{});reader?.releaseLock();}
}

export async function connectNetwork(options:{identity:ControlIdentity;trackUrl:string;signal:AbortSignal;timeoutMs:number}):Promise<NetworkSession> {
  const base=endpoint(options.trackUrl,'http');base.pathname=base.pathname.replace(/\/$/,'')+'/';
  const {identity}=options;
  const controller=new AbortController();const abort=()=>controller.abort();
  options.signal.addEventListener('abort',abort,{once:true});if(options.signal.aborted)abort();
  let socket:PunchSocket|undefined;
  let bindingKey:Uint8Array|undefined;let trackKey:Uint8Array|undefined;let bindingId='';
  let punchSeq=0n;let trackSeq=0n;let timer:ReturnType<typeof setTimeout>|undefined;
  let cleanupParent=()=>{};
  let closing:Promise<void>|undefined;let beating:Promise<void>|undefined;
  let interval=30000;let iceServers:RTCIceServer[]=[];let renewal=0;
  const publications=new Map<string,UploadHandler>();const completePublications=new Set<string>();let assigned:{punch_id:string;public_key:string;endpoints:string[]}|undefined;
  const controlPool=new PermitPool(1);const connections=new Set<PeerConnection>();const dials=new Set<Promise<PeerConnection>>();const queries=new Set<Promise<PeerCandidate[]>>();
  let snapshot:Readonly<NetworkSnapshot>=Object.freeze({state:'connecting',peerId:identity.info.peer_id,punchId:null,error:null});
  const listeners=new Set<(s:Readonly<NetworkSnapshot>)=>void>();
  function emit(state:NetworkSnapshot['state'],error?:Q2QError) {
    snapshot=Object.freeze({...snapshot,state,error:error?Object.freeze({code:error.code,retryable:error.retryable}):null});
    for(const listener of listeners){try{void Promise.resolve(listener(snapshot)).catch(()=>{});}catch{/* isolated observer */}}
  }
  function fail(error:Q2QError) {
    if(closing||snapshot.state==='closed'||snapshot.state==='offline')return;
    clearTimeout(timer);controller.abort();emit('offline',error);
    void socket?.close();
  }
  function sequence(value:bigint){if(value>=(1n<<64n)-1n)throw new Q2QError('RESOURCE_LIMIT');return value+1n;}
  const request=(path:string,body:string|undefined,method:'PUT'|'POST'|'GET')=>jsonRequest(new URL(path,base),body,method,controller.signal,options.timeoutMs);
  const schemes = globalThis.location?.protocol==='http:' && ['localhost','127.0.0.1','[::1]'].includes(globalThis.location.hostname) ? ['wss','ws'] : ['wss'];
  function assignment(value: unknown) {
    const parsed=punchAssignmentSchema.safeParse(value);
    if(!parsed.success)throw new Q2QError('NETWORK_PROTOCOL_ERROR');
    const urls=parsed.data.endpoints.filter(url=>schemes.includes(url.split(':')[0]!)).sort((a,b)=>Number(b.startsWith('wss:'))-Number(a.startsWith('wss:')));
    const url=urls.find(url=>{try{endpoint(url,'websocket');return true;}catch{return false;}});
    return {...parsed.data, websocketUrl:url};
  }
  try {
    const value=await request(`peers/${identity.info.peer_id}/files`,`{"token":${identity.announce()},"ttl":300,"punch_schemes":${JSON.stringify(schemes)},"add":[],"del":[],"complete":[]}`,'PUT');
    trackKey=decodeKey(value.session_key);expiry(value.expires_at);
    interval=integer(value.heartbeat_interval,1,300)*1000;
    const punch=assignment(value.punch);
    if(!punch.websocketUrl)throw new Q2QError('PUNCH_UNAVAILABLE');
    // TURN is intentionally not exposed in a public snapshot. RTC consumes it later.
    const turn=value.turn as {url?:unknown;username?:unknown;credential?:unknown;expires_at?:unknown}|undefined;
    if(!turn||typeof turn.url!=='string'||!/^turns?:/.test(turn.url)||turn.url.length>2048||typeof turn.username!=='string'||turn.username.length>256||typeof turn.credential!=='string'||turn.credential.length>512)throw new Q2QError('NETWORK_PROTOCOL_ERROR');
    expiry(turn.expires_at);iceServers=[{urls:turn.url,username:turn.username,credential:turn.credential}];
    assigned=punch;
    snapshot=Object.freeze({...snapshot,punchId:punch.punch_id});
    socket=await PunchSocket.open({url:punch.websocketUrl,identity,signal:controller.signal,timeoutMs:options.timeoutMs,onFailure:fail,
      onSignal:async message=>{
        // No publication/RTC acceptance yet: reject unsolicited connections with
        // the existing typed protocol, rather than claim a peer is authenticated.
        if(!socket)return;
        const context:UploadContext={identity,punchId:assigned!.punch_id,iceServers,signal:controller.signal,
          send:value=>socket!.rpc('punch.signal',JSON.stringify(value),controller.signal,options.timeoutMs)};
        if(message.type==='connect'){
          if(typeof message.connect_token!=='string')return;
          try{
            const ticket=identity.inspectConnect(message.connect_token,assigned!.punch_id,message.connection_id);
            const handler=publications.get(`sha256:${ticket.info_hash}`);
            if(handler){await handler.signal(message,context);return;}
          }catch{/* Bad tickets must never reach publication state. */}
          await context.send({type:'reject',connection_id:message.connection_id,reason:'busy'}).catch(()=>{});
        }else{await Promise.all([...publications.values()].map(handler=>handler.signal(message,context)));}
      }});
    const joined=await socket.rpc('punch.join',identity.join(punch.punch_id),controller.signal,options.timeoutMs) as {binding_id?:unknown;binding_key?:unknown;expires_at?:unknown};
    if(typeof joined.binding_id!=='string'||!/^[A-Za-z0-9_.-]{1,128}$/.test(joined.binding_id))throw new Q2QError('NETWORK_PROTOCOL_ERROR');
    bindingId=joined.binding_id;bindingKey=decodeKey(joined.binding_key);
    interval=Math.min(interval,Math.max(1000,(expiry(joined.expires_at)-Number(now()))*1000/3));
    if(controller.signal.aborted)throw new Q2QError('TASK_CANCELLED');
  }catch(error){
    controller.abort();await socket?.close();trackKey?.fill(0);bindingKey?.fill(0);options.signal.removeEventListener('abort',abort);
    throw error instanceof Q2QError?error:new Q2QError('NETWORK_PROTOCOL_ERROR');
  }
  async function announceFiles(add:string[],del:string[]){
    const release=await controlPool.acquire(controller.signal);
    try{
      const value=await request(`peers/${identity.info.peer_id}/files`,JSON.stringify({token:JSON.parse(identity.announce()),ttl:300,punch_schemes:schemes,add,del,complete:add.filter(hash=>completePublications.has(hash))}),'PUT');
      const next=assignment(value.punch);
      if(next.punch_id!==assigned!.punch_id||next.public_key!==assigned!.public_key||JSON.stringify(next.endpoints)!==JSON.stringify(assigned!.endpoints)){
        const error=new Q2QError('NETWORK_PROTOCOL_ERROR');fail(error);throw error;
      }
      trackKey?.fill(0);trackKey=decodeKey(value.session_key);trackSeq=0n;expiry(value.expires_at);renewal=Date.now();
    }finally{release();}
  }
  // A parent Client close remains authoritative for the lifetime of this session.
  function schedule(){clearTimeout(timer);if(snapshot.state==='online')timer=setTimeout(()=>{void session.heartbeat().catch(()=>{});},interval);}
  const session:NetworkSession=Object.freeze({
    snapshot:()=>snapshot,
    subscribe(listener:(s:Readonly<NetworkSnapshot>)=>void){if(snapshot.state!=='closed')listeners.add(listener);try{void Promise.resolve(listener(snapshot)).catch(()=>{});}catch{}return()=>{listeners.delete(listener);};},
    heartbeat(){
      if(closing||controller.signal.aborted||snapshot.state!=='online')return Promise.reject(new Q2QError('NETWORK_CLOSED'));
      if(beating)return beating;
      clearTimeout(timer);
      const work=(async()=>{
        try {
          if(publications.size&&Date.now()-renewal>=120000)await announceFiles([...publications.keys()],[]);
          const release=await controlPool.acquire(controller.signal);
          try{trackSeq=sequence(trackSeq);const result=await request(`peers/${identity.info.peer_id}/heartbeat`,identity.trackAuth(trackKey!,trackSeq),'POST');expiry(result.expires_at);}finally{release();}
          punchSeq=sequence(punchSeq);
          const response=await socket!.rpc('punch.heartbeat',identity.bindingAuth(bindingKey!,bindingId,punchSeq),controller.signal,options.timeoutMs) as {expires_at?:unknown};expiry(response.expires_at);
        }catch(error){const mapped=error instanceof Q2QError?error:new Q2QError('NETWORK_PROTOCOL_ERROR');if(!closing)fail(mapped);throw mapped;}
      })();
      beating=work.finally(()=>{beating=undefined;schedule();});return beating;
    },
    close(){
      if(closing)return closing;
      clearTimeout(timer);controller.abort();
      closing=(async()=>{
        try {
          await beating?.catch(()=>{});
          await Promise.allSettled([...publications.values()].map(handler=>handler.close()));publications.clear();
          await Promise.allSettled([...dials,...queries]);
          await Promise.allSettled([...connections].map(peer=>peer.close()));connections.clear();
          if(bindingKey&&socket){
            punchSeq=sequence(punchSeq);
            const exit=new AbortController();
            await socket.rpc('punch.exit',identity.bindingAuth(bindingKey,bindingId,punchSeq,true),exit.signal,Math.min(1000,options.timeoutMs)).catch(()=>{});
          }
        }finally{
          await socket?.close();iceServers=[];trackKey?.fill(0);bindingKey?.fill(0);options.signal.removeEventListener('abort',abort);cleanupParent();emit('closed');listeners.clear();
        }
      })();return closing;
    }
  });
  const parentClose=()=>{void session.close().catch(()=>{});};
  options.signal.addEventListener('abort',parentClose,{once:true});
  const originalClose=session.close;
  // The signal listener is removed by close's cleanup below; it must not retain
  // old sessions across explicit reconnects.
  cleanupParent=()=>options.signal.removeEventListener('abort',parentClose);
  if(options.signal.aborted){await originalClose();throw new Q2QError('TASK_CANCELLED');}
  peers.set(session,{
    async publish(hash,handler,complete=true){
      if(closing||controller.signal.aborted)throw new Q2QError('NETWORK_CLOSED');
      if(publications.size||!/^sha256:[a-f0-9]{64}$/.test(hash))throw new Q2QError('RESOURCE_LIMIT');
      publications.set(hash,handler);if(complete)completePublications.add(hash);
      try{await announceFiles([hash],[]);}catch(error){publications.delete(hash);completePublications.delete(hash);throw error;}
      let stopped:Promise<void>|undefined;
      return ()=>stopped??=(async()=>{
        publications.delete(hash);completePublications.delete(hash);await handler.close();
        if(!controller.signal.aborted)await announceFiles([],[hash]);
      })();
    },
    query(hash,signal){
      if(closing||controller.signal.aborted||snapshot.state!=='online')return Promise.reject(new Q2QError('NETWORK_CLOSED'));
      const work=(async()=>{
      if(!/^sha256:[a-f0-9]{64}$/.test(hash))throw new Q2QError('INVALID_ARGUMENT');
      const combined=AbortSignal.any([signal,controller.signal]);const result:PeerCandidate[]=[];let cursor:string|undefined;
      for(let page=0;page<4;page++){
        const release=await controlPool.acquire(combined);
        try{
          trackSeq=sequence(trackSeq);const auth=JSON.parse(identity.trackAuth(trackKey!,trackSeq)) as {mac:string;timestamp:number};
          const url=new URL(`files/${hash}/peers`,base);url.search=new URLSearchParams({peer_id:identity.info.peer_id,seq:trackSeq.toString(),timestamp:String(auth.timestamp),mac:auth.mac,limit:'10',...(cursor?{cursor}:{})}).toString();
          const value=await jsonRequest(url,undefined,'GET',combined,options.timeoutMs);
          if(!Array.isArray(value.peers)||value.peers.length>10)throw new Q2QError('NETWORK_PROTOCOL_ERROR');
          for(const item of value.peers){
            if(!item||typeof item!=='object')throw new Q2QError('NETWORK_PROTOCOL_ERROR');
            const candidate=item as {peer_id?:unknown;public_key?:unknown;punch?:unknown;complete?:unknown};
            const publicKey=decodeKey(candidate.public_key), punch=assignment(candidate.punch);
            if(typeof candidate.peer_id!=='string'||identity.peerId(publicKey)!==candidate.peer_id||typeof candidate.complete!=='boolean')throw new Q2QError('NETWORK_PROTOCOL_ERROR');
            if(candidate.peer_id===identity.info.peer_id||!punch.websocketUrl)continue;
            if(!result.some(peer=>peer.peerId===candidate.peer_id))result.push({peerId:candidate.peer_id,publicKey,punchId:punch.punch_id,websocketUrl:punch.websocketUrl,complete:candidate.complete});
          }
          if(value.next_cursor===null)break;
          if(typeof value.next_cursor!=='string'||value.next_cursor.length>1024||value.next_cursor===cursor)throw new Q2QError('NETWORK_PROTOCOL_ERROR');cursor=value.next_cursor;
        }finally{release();}
      }
      return result;
      })();queries.add(work);void work.finally(()=>queries.delete(work)).catch(()=>{});return work;
    },
    dial(target,hash,signal){
      if(closing||controller.signal.aborted||snapshot.state!=='online')return Promise.reject(new Q2QError('NETWORK_CLOSED'));
      const work=dialPeer({identity,target,hash,iceServers,signal:AbortSignal.any([controller.signal,signal]),timeoutMs:Math.min(20000,options.timeoutMs*2)}).then(peer=>{
        const owned={rtc:peer.rtc,async close(){try{await peer.close();}finally{connections.delete(owned);}}};connections.add(owned);return owned;
      });dials.add(work);void work.finally(()=>dials.delete(work)).catch(()=>{});return work;
    }
  });
  emit('online');schedule();return session;
}
