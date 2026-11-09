import type {LiveBlocks} from './partial-sharing.ts';
import {hasBlock} from './checkpoint-codec.ts';
import { Q2QError } from './error.ts';
import { initializeWasm } from './block-verifier.ts';
import { decodeKey } from './control-identity.ts';
import type { ControlIdentity } from './control-identity.ts';
import type { SignalMessage } from './punch-socket.ts';
import { RtcPeer, Inbox } from './rtc-peer.ts';
import type { SourceReader } from './storage.ts';
import type { Seed } from './client-types.ts';
import { PermitPool } from './http.ts';

type Wasm=Awaited<ReturnType<typeof initializeWasm>>;
export interface UploadContext {
 identity:ControlIdentity;punchId:string;iceServers:RTCIceServer[];signal:AbortSignal;
 send(value:unknown):Promise<unknown>;
}
export interface UploadHandler {signal(message:SignalMessage,context:UploadContext):Promise<void>;close():Promise<void>}
const encode=(value:unknown)=>JSON.stringify({v:1,message:value});

/** Only one bounded block is retained per authenticated uploader. Source leases
 * are independent; no shared mutable file cursor or full-file buffering. */
export function uploadHandler(options:{seed:Seed;source:{open():Promise<SourceReader>};proofs?:InstanceType<Wasm['UploadProofs']>;blocks?:LiveBlocks;wasm:Wasm;
 signal:AbortSignal;permits:PermitPool;reads:PermitPool;maxConnections:number;connections(delta:number):void;sent(bytes:number):void;fatal(error:unknown):void}):UploadHandler {
 const peers=new Map<string,{controller:AbortController;inbox:Inbox<SignalMessage>;work:Promise<void>}>();let closing:Promise<void>|undefined;
 async function serve(first:SignalMessage,context:UploadContext,inbox:Inbox<SignalMessage>,signal:AbortSignal){
  const release=await options.permits.acquire(signal);let rtc:RtcPeer|undefined;let reader:SourceReader|undefined;let pump:Promise<void>|undefined;let counted=false;let unsubscribe:(()=>void)|undefined;const notifications=new Set<Promise<void>>();
  const controller=new AbortController(),combined=AbortSignal.any([signal,context.signal,controller.signal]);
  const send=(value:unknown)=>context.send(value);
  try{
   const remote=context.identity.inspectConnect(first.connect_token!,context.punchId,first.connection_id);
   const hash=Uint8Array.from(remote.info_hash.match(/../g)!,value=>parseInt(value,16));
   if(`sha256:${remote.info_hash}`!==options.seed.info.id)throw new Q2QError('PEER_AUTH_FAILED');
   const accept=context.identity.connectionTicket(context.punchId,remote.peer_id,hash,first.connection_id);
   await send({type:'accept',connection_id:first.connection_id,accept_token:accept});
   rtc=await RtcPeer.create({identity:context.identity,remoteKey:decodeKey(remote.public_key),infoHash:hash,connectionId:first.connection_id,initiator:false,
    signal:combined,timeoutMs:20000,iceServers:context.iceServers,onCandidate:candidate=>send({type:'candidate',connection_id:first.connection_id,candidate:JSON.stringify(candidate)}).then(()=>{})});
   let offered=false;
   pump=(async()=>{for(;;){const message=await inbox.read(combined);
    if(message.type==='offer'&&!offered&&typeof message.sdp==='string'){
     offered=true;if(message.connect_token!==first.connect_token)throw new Q2QError('PEER_AUTH_FAILED');
     const answer=await rtc!.answer(JSON.parse(message.sdp) as RTCSessionDescriptionInit);
     await send({type:'answer',connection_id:first.connection_id,sdp:JSON.stringify(answer)});
    }else if(message.type==='candidate'&&typeof message.candidate==='string')await rtc!.addCandidate(JSON.parse(message.candidate) as RTCIceCandidateInit);
    else throw new Q2QError('PEER_PROTOCOL_ERROR');
   }})();void pump.catch(()=>controller.abort());
   await rtc.ready();options.connections(1);counted=true;
   const count=Number(options.seed.info.blockCount),bits=options.blocks?.bitmap()??new Uint8Array(Math.ceil(count/8));if(!options.blocks)for(let i=0;i<count;i++)bits[i>>3]!|=1<<(i%8);
   const firstRaw=await rtc.readControl(AbortSignal.any([combined,AbortSignal.timeout(10000)]));
   options.wasm.peer_bitfield(firstRaw,count);
   await rtc.sendControl(options.wasm.encode_peer_bitfield(bits,count),combined);
   // Await actual host open even when cancelled: keep ownership until it settles.
   if(options.blocks){
    unsubscribe=options.blocks.subscribe(index=>{
      if(notifications.size>=32){controller.abort();return;}
      const work=rtc!.sendControl(encode({type:'have',block_index:index}),combined);notifications.add(work);
      void work.catch(()=>controller.abort()).finally(()=>notifications.delete(work));
    });
    const current=options.blocks.bitmap();for(let i=0;i<count;i++)if(hasBlock(current,i)&&!hasBlock(bits,i))await rtc.sendControl(encode({type:'have',block_index:i}),combined);
   }else{try{reader=await options.source.open();}catch(error){options.fatal(error);throw error;}}
   if(combined.aborted)throw new Q2QError('TASK_CANCELLED');
   let active:{index:number;bytes:Uint8Array;proof:string}|undefined;
   const control=(message:unknown)=>rtc!.sendControl(encode(message),combined);
   async function pieces(index:number,bytes:Uint8Array,start=0,length=bytes.length){
    for(let offset=start;offset<start+length;offset+=1024){const data=bytes.subarray(offset,Math.min(offset+1024,start+length));await rtc!.sendPiece(BigInt(index),offset,data,combined);options.sent(data.length);}
   }
   for(;;){
    const raw=await rtc.readControl(AbortSignal.any([combined,AbortSignal.timeout(30000)]));
    const message=JSON.parse(options.wasm.decode_peer_control(raw)) as {type:string;block_index?:string;count?:number;piece_offset?:number[];length?:number[]};
    const index=message.block_index===undefined?-1:Number(BigInt(message.block_index));
    if(message.type==='cancel'&&index===0&&message.count===0){await control({type:'cancel',block_index:0,count:0});break;}
    if(message.type==='have')continue;
    if(message.type==='block_done'){if(active?.index===index)active=undefined;continue;}
    if(!Number.isSafeInteger(index)||index<0||index>=count)throw new Q2QError('PEER_PROTOCOL_ERROR');
    if(options.blocks&&!hasBlock(options.blocks.bitmap(),index)){
      await control({type:'reject',block_index:index,count:1,retry_after:1000});continue;
    }
    if(message.type==='request'&&message.count===1){
     if(active&&active.index!==index)throw new Q2QError('PEER_PROTOCOL_ERROR');
     if(!active){const releaseRead=await options.reads.acquire(combined);try{
      const offset=BigInt(index)*BigInt(options.seed.info.blockSize),length=Number(options.seed.info.size-offset<BigInt(options.seed.info.blockSize)?options.seed.info.size-offset:BigInt(options.seed.info.blockSize));
      const shared=options.blocks?await options.blocks.read(index).catch(error=>{if(!combined.aborted)options.fatal(error);throw error;}):undefined;
      if(options.blocks&&!shared)throw new Q2QError('SOURCE_UNAVAILABLE');
      const bytes=shared?shared.bytes:await reader!.read(offset,length,{signal:combined}).catch(error=>{if(!combined.aborted)options.fatal(error);throw error;});
      if(combined.aborted)throw new Q2QError('TASK_CANCELLED');
      if(!shared&&!options.proofs!.verify(index,bytes)){const error=new Q2QError('SOURCE_CHANGED');options.fatal(error);throw error;}
      active={index,bytes,proof:shared?.proof??options.proofs!.proof(index)};
     }finally{releaseRead();}}
     await rtc.sendControl(active.proof,combined);await pieces(index,active.bytes);
    }else if(message.type==='request_proof'&&message.count===1){const shared=options.blocks?await options.blocks.read(index).catch(error=>{if(!combined.aborted)options.fatal(error);throw error;}):undefined;await rtc.sendControl(shared?.proof??options.proofs!.proof(index),combined);}
    else if(message.type==='piece_nack'&&active?.index===index){
     for(let i=0;i<message.piece_offset!.length;i++){const start=message.piece_offset![i]!,length=message.length![i]!;
      if(start%1024!==0||start+length>active.bytes.length)throw new Q2QError('PEER_PROTOCOL_ERROR');await pieces(index,active.bytes,start,length);}
    }else if(message.type==='cancel'&&message.count===1){active=undefined;}
    else throw new Q2QError('PEER_PROTOCOL_ERROR');
   }
  }finally{
   controller.abort();unsubscribe?.();inbox.close(new Q2QError('PEER_CLOSED'));
   try{await rtc?.close();await Promise.allSettled([...notifications]);await pump?.catch(()=>{});await reader?.close();}
   finally{if(counted)options.connections(-1);release();}
  }
 }
 return {
  async signal(message,context){
   if(closing||options.signal.aborted)return;
   const existing=peers.get(message.connection_id);
   if(message.type!=='connect'){if(existing){try{existing.inbox.push(message,JSON.stringify(message).length);}catch{existing.controller.abort();}}return;}
   if(existing)return;
   if(peers.size>=options.maxConnections){await context.send({type:'reject',connection_id:message.connection_id,reason:'busy'});return;}
   if(typeof message.connect_token!=='string')return;
   const controller=new AbortController(),inbox=new Inbox<SignalMessage>();
   const work=serve(message,context,inbox,AbortSignal.any([options.signal,controller.signal])).catch(()=>{}).finally(()=>{peers.delete(message.connection_id);});
   peers.set(message.connection_id,{controller,inbox,work});
  },
  close(){closing??=(async()=>{for(const peer of peers.values())peer.controller.abort();await Promise.allSettled([...peers.values()].map(peer=>peer.work));})();return closing;}
 };
}
