import { initializeWasm } from './block-verifier.ts';
import { Q2QError } from './error.ts';
import { abortable } from './http.ts';
export const now = () => BigInt(Math.floor(Date.now()/1000));
export function decodeKey(value:unknown):Uint8Array {
  if(typeof value!=='string'||!/^[A-Za-z0-9+/]{43}=$/.test(value))throw new Q2QError('NETWORK_PROTOCOL_ERROR');
  const bytes=Uint8Array.from(atob(value),c=>c.charCodeAt(0));
  if(bytes.length!==32||btoa(String.fromCharCode(...bytes))!==value)throw new Q2QError('NETWORK_PROTOCOL_ERROR');
  return bytes;
}
export function endpoint(value:string,kind:'http'|'websocket'):URL {
  let url:URL;try{url=new URL(value);}catch{throw new Q2QError('INVALID_ARGUMENT');}
  const loopback=['localhost','127.0.0.1','[::1]'];
  const local=loopback.includes(url.hostname)&&globalThis.location?.protocol==='http:'&&loopback.includes(globalThis.location.hostname);
  const secure=kind==='http'?'https:':'wss:';
  const plain=kind==='http'?'http:':'ws:';
  if(url.username||url.password||(kind==='http'&&url.search)||url.hash||!url.hostname||url.href.length>2048
    ||url.protocol!==secure&&!(local&&url.protocol===plain))throw new Q2QError('INVALID_ARGUMENT');
  return url;
}
export async function controlIdentity(wasmUrl?:string|URL,signal?:AbortSignal) {
  const module=signal?await abortable(initializeWasm(wasmUrl),signal):await initializeWasm(wasmUrl);
  const entropy=crypto.getRandomValues(new Uint8Array(32));
  let key:InstanceType<typeof module.ControlIdentity>;
  try{key=new module.ControlIdentity(entropy);}finally{entropy.fill(0);}
  const info=Object.freeze(JSON.parse(key.info()) as {peer_id:string;public_key:string});
  let closed=false;
  const check=()=>{if(closed)throw new Q2QError('NETWORK_CLOSED');};
  return Object.freeze({
    info,
    announce:()=>{check();return key.announce(now());},
    join:(punch:string)=>{check();return key.join(punch,crypto.randomUUID(),now());},
    connectionTicket:(punch:string,target:string,hash:Uint8Array,id:string)=>{check();return key.connect_ticket(punch,target,hash,id,now());},
    trackAuth:(session:Uint8Array,seq:bigint)=>{check();return key.track_auth(session,seq,now());},
    bindingAuth:(binding:Uint8Array,id:string,seq:bigint,exit=false)=>{check();return module.punch_binding_auth(binding,id,seq,now(),0n,exit);},
    peerId:(remote:Uint8Array)=>{check();return key.peer_id_for_key(remote);},
    inspectConnect:(raw:string,punch:string,connection:string)=>{
      check();try{return JSON.parse(key.inspect_connect(raw,punch,connection,now())) as {peer_id:string;public_key:string;info_hash:string};}
      catch{throw new Q2QError('PEER_AUTH_FAILED');}
    },
    verifyAccept:(raw:string,remote:Uint8Array,connection:string,punch:string,hash:Uint8Array)=>{check();try{key.verify_accept(raw,remote,connection,punch,hash,now());}catch{throw new Q2QError('PEER_AUTH_FAILED');}},
    beginPeer:(remote:Uint8Array,initiator:boolean,hash:Uint8Array,connection:string,localSdp:string,remoteSdp:string)=>{
      check();const nonce=crypto.getRandomValues(new Uint8Array(32));
      try{return key.begin_peer(remote,initiator,hash,connection,nonce,localSdp,remoteSdp);}finally{nonce.fill(0);}
    },
    decodeFrame:(bytes:Uint8Array)=>{check();return module.decode_punch_frame(bytes);},
    close:()=>{if(!closed){closed=true;key.free();}}
  });
}
export type ControlIdentity = Awaited<ReturnType<typeof controlIdentity>>;
