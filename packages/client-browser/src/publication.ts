import type {LiveBlocks} from './partial-sharing.ts';
import {abortable} from './http.ts';
import { Q2QError } from './error.ts';
import { initializeWasm } from './block-verifier.ts';
import { parseSeed } from './seed.ts';
import { peerNetwork } from './network.ts';
import { uploadHandler } from './peer-upload.ts';
import { PermitPool } from './http.ts';
import type { NetworkSession } from './network.ts';
import type { PublishOptions,Publication,PublicationSnapshot,Seed } from './client-types.ts';
import type { SourceReader } from './storage.ts';

type Wasm=Awaited<ReturnType<typeof initializeWasm>>;
export function createPublication(options:PublishOptions&{preparedSeed?:Seed;blocks?:LiveBlocks;wasmUrl?:string|URL;connect():Promise<NetworkSession>;permits:PermitPool;reads:PermitPool;maxConnections:number;ended():void}):Publication {
 const id=crypto.randomUUID(),started=performance.now(),size=options.source.info.size;
 let snapshot:Readonly<PublicationSnapshot>=Object.freeze({id,state:'preparing',name:options.name??options.source.info.name,totalBytes:size,contentId:null,preparedBytes:0n,sentBytes:0n,bytesPerSecond:0,activeConnections:0,error:null});
 let lastNotification=0;
 const listeners=new Set<(value:Readonly<PublicationSnapshot>)=>void>();
 let seed:Seed|undefined=options.preparedSeed,wasm:Wasm|undefined,proofs:InstanceType<Wasm['UploadProofs']>|undefined;
 let controller=new AbortController(),work:Promise<void>=Promise.resolve(),withdraw:(()=>Promise<void>)|undefined;
 let removeBlockListener=()=>{};
 let closing:Promise<void>|undefined,pausing:Promise<void>|undefined,networkUnsubscribe:(()=>void)|undefined;
 let resolveReady!:(value:{seed:Seed})=>void,rejectReady!:(error:unknown)=>void;
 const ready=new Promise<{seed:Seed}>((resolve,reject)=>{resolveReady=resolve;rejectReady=reject;});void ready.catch(()=>{});
 function emit(value:Partial<PublicationSnapshot>,important=true){snapshot=Object.freeze({...snapshot,...value});if(!important&&performance.now()-lastNotification<100)return;lastNotification=performance.now();for(const listener of listeners){try{void Promise.resolve(listener(snapshot)).catch(()=>{});}catch{}}}
 function stopped(){if(controller.signal.aborted)throw new Q2QError('TASK_CANCELLED');}
 async function release(){networkUnsubscribe?.();networkUnsubscribe=undefined;const own=withdraw;withdraw=undefined;await own?.();}
 async function fail(value:unknown){
  if(closing||snapshot.state==='pausing'||snapshot.state==='paused'||snapshot.state==='failed')return;
  const error=value instanceof Q2QError?value:new Q2QError('SOURCE_UNAVAILABLE');
  controller.abort();emit({state:'failed',error:{code:error.code,retryable:error.retryable,...(error.diagnostic?{diagnostic:error.diagnostic}:{})}});rejectReady(error);
  try{await release();}finally{proofs?.free();proofs=undefined;options.ended();}
 }
 async function prepare(){
  wasm=await initializeWasm(options.wasmUrl);stopped();
  let builder:InstanceType<Wasm['SeedBuilder']>|undefined=new wasm.SeedBuilder(Number(size));let reader:SourceReader|undefined;
  try{
   reader=await options.source.open();stopped();emit({preparedBytes:0n});
   for(let offset=0n;offset<size;offset+=262144n){
    stopped();const length=Number(size-offset<262144n?size-offset:262144n);
    const bytes=await reader.read(offset,length,{signal:controller.signal});stopped();
    if(bytes.length!==length)throw new Q2QError('SOURCE_CHANGED');builder.update(bytes);emit({preparedBytes:offset+BigInt(length)},false);
   }
   stopped();const prepared=builder.finish_inline(snapshot.name??'file');builder=undefined;
   let parsed:Seed;
   try{parsed=await parseSeed(prepared.seed_bytes(),controller.signal,options.wasmUrl);}finally{prepared.free();}
   stopped();proofs=new wasm.UploadProofs(parsed.toBytes());seed=parsed;emit({contentId:seed.info.id});
  }finally{builder?.free();await reader?.close();}
 }
 async function start(){
  try{
   if(options.blocks){wasm=await initializeWasm(options.wasmUrl);await abortable(options.blocks.ready,controller.signal);emit({contentId:seed!.info.id});}
   else if(!seed)await prepare();stopped();
   const network=await options.connect();stopped();
   const handler=uploadHandler({seed:seed!,source:options.source,proofs,blocks:options.blocks,wasm:wasm!,signal:controller.signal,permits:options.permits,reads:options.reads,maxConnections:options.maxConnections,
    connections:delta=>emit({activeConnections:snapshot.activeConnections+delta}),
    sent:bytes=>{const sentBytes=snapshot.sentBytes+BigInt(bytes);emit({sentBytes,bytesPerSecond:Number(sentBytes)/Math.max(.001,(performance.now()-started)/1000)},false);},
    fatal:error=>{void fail(error).catch(()=>{});}});
   try{withdraw=await peerNetwork(network).publish(seed!.info.id,handler,options.blocks===undefined);}catch(error){await handler.close();throw error;}
   stopped();networkUnsubscribe=network.subscribe(value=>{if(value.state==='offline'||value.state==='closed')void fail(new Q2QError('NETWORK_CLOSED')).catch(()=>{});});
   emit({state:'publishing'});resolveReady(Object.freeze({seed:seed!}));
  }catch(error){if(!controller.signal.aborted)await fail(error);}
 }
 work=Promise.resolve().then(start);void work.catch(error=>{void fail(error).catch(()=>{});});
 const publication:Publication=Object.freeze({id,snapshot:()=>snapshot,
  subscribe(listener:(value:Readonly<PublicationSnapshot>)=>void){if(snapshot.state!=='closed')listeners.add(listener);try{void Promise.resolve(listener(snapshot)).catch(()=>{});}catch{}return()=>{listeners.delete(listener);};},
  ready:()=>ready,
  pause(){
   if(closing||snapshot.state==='closed'||snapshot.state==='failed')return Promise.reject(new Q2QError('INVALID_STATE'));
   if(pausing)return pausing;if(snapshot.state==='paused')return Promise.resolve();
   pausing=Promise.resolve().then(async()=>{await work;await release();if(!closing)emit({state:'paused'});}).finally(()=>{pausing=undefined;});
   emit({state:'pausing'});controller.abort();return pausing;
  },
  async resume(){
   if(closing||snapshot.state==='closed'||snapshot.state==='failed')throw new Q2QError('INVALID_STATE');
   await pausing;if(snapshot.state!=='paused')return;
   controller=new AbortController();emit({state:'preparing'});work=start();await work;
   if(publication.snapshot().state==='failed')throw new Q2QError(publication.snapshot().error!.code);
  },
  close(){
   if(closing)return closing;
   closing=Promise.resolve().then(async()=>{try{await work;await pausing;await release();emit({state:'closed'});}finally{proofs?.free();proofs=undefined;removeBlockListener();options.ended();listeners.clear();}});
   emit({state:'closing'});controller.abort();rejectReady(new Q2QError('TASK_CANCELLED'));return closing;
  }
 });
 if(options.blocks){
  const end=()=>{void publication.close().catch(()=>{});};
  options.blocks.signal.addEventListener('abort',end,{once:true});
  removeBlockListener=()=>options.blocks!.signal.removeEventListener('abort',end);
 }
 return publication;
}
