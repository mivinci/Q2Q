import {createClient} from '../src/index.ts';
import {initializeWasm} from '../src/block-verifier.ts';
import {memoryState} from '../src/memory-state.ts';
import {hasBlock} from '../src/checkpoint-codec.ts';
import {peerNetwork} from '../src/network.ts';
import type {Client,Publication,Transfer} from '../src/client-types.ts';
let client:Client,publication:Publication,transfer:Transfer;let restartOptions:Parameters<Client['downloadShared']>[0];
let corruptBytes=()=>{};let restorePieces=()=>{};let releasePieces=()=>{};let cleanupTarget:()=>Promise<void>=async()=>{};let releaseMetadata:()=>void=()=>{};let metadataWaiting=false;
const assert=(value:unknown,message='partial assertion failed')=>{if(!value)throw Error(message);};
export async function start(trackUrl:string,mode='memory',suppliedSeed?:string){
 const wasm=await initializeWasm(),bytes=new Uint8Array(3*262144+17);for(let i=0;i<bytes.length;i++)bytes[i]=(i*31+Math.floor(i/262144))%251;
 const builder=new wasm.SeedBuilder(bytes.length);builder.update(bytes);const prepared=builder.finish_inline('partial.bin');
 const state=memoryState(),originalLease=state.lease.bind(state);
 const gate=new Promise<void>(resolve=>{releaseMetadata=resolve;});
 state.lease; // memory store is immutable; inject a wrapping store, never change its rows.
 const wrapped={...state,async lease(id:string){const lease=await originalLease(id);return{...lease,async replace(revision:number,bits:Uint8Array,witnesses?:ReadonlyMap<number,Uint8Array>){
  if(hasBlock(bits,0)&&!metadataWaiting){metadataWaiting=true;await gate;if(mode==='metadata-fail')throw new Error('injected checkpoint failure');}
  return lease.replace(revision,bits,witnesses);
 }};}};
 const destinationBytes=new Uint8Array(bytes.length);corruptBytes=()=>{destinationBytes[0]!^=1;};let targetId=crypto.randomUUID();
 const destination={targetId,async open(){return{
  async read(offset:bigint,length:number){return destinationBytes.slice(Number(offset),Number(offset)+length);},
  async write(offset:bigint,value:Uint8Array){destinationBytes.set(value,Number(offset));},async commit(){},
  async finish(){return{kind:'staged' as const,targetId,needsExport:true as const};},async abort(){}
 };}};
 if(mode==='sparse'){
  const {RtcPeer}=await import('../src/rtc-peer.ts');const original=RtcPeer.prototype.readPiece;
  const pieceGate=new Promise<void>(resolve=>{releasePieces=resolve;});
  RtcPeer.prototype.readPiece=async function(signal){const piece=await original.call(this,signal);if(piece.blockIndex>0n){const {abortable}=await import('../src/http.ts');await abortable(pieceGate,signal);}return piece;};
  restorePieces=()=>{RtcPeer.prototype.readPiece=original;};
 }
 client=createClient({trackUrl,state:wrapped,...(mode==='sparse'?{}:{sourceProvider:{async resolve(){return{contentUrls:[`${location.origin}/partial-content`]};}}}),limits:{httpRequests:1,commitBytes:262144}});
 let selected=destination;
 if(mode==='opfs'){
  const {opfsCache}=await import('../src/index.ts');const cache=await opfsCache();const entry=await cache.create({name:'partial-opfs.bin',size:BigInt(bytes.length)});
  selected=entry.destination as typeof destination;cleanupTarget=()=>cache.remove(entry.id);
 }
 const seed=await client.parseSeed(suppliedSeed?Uint8Array.from(atob(suppliedSeed),c=>c.charCodeAt(0)):prepared.seed_bytes());prepared.free();
 restartOptions={seed,destination:selected};const handles=client.downloadShared(restartOptions);publication=handles.publication;transfer=handles.transfer;
 await publication.ready();
 return {seed:Array.from(seed.toBytes()),id:seed.info.id};
}
export async function waiting(){return metadataWaiting;}
export async function commit(){releaseMetadata();}
export async function finish(){releasePieces();await transfer.wait();restorePieces();assert(publication.snapshot().state==='closed');await client.close();await cleanupTarget();}
export async function close(){releaseMetadata();releasePieces();restorePieces();await publication?.close();await transfer?.cancel();await client?.close();await cleanupTarget().catch(()=>{});}
let other:Client,connection:Awaited<ReturnType<ReturnType<typeof peerNetwork>['dial']>>,available:Uint8Array;
export async function probe(trackUrl:string,id:string,recovered=false){
 const wasm=await initializeWasm();other=createClient({trackUrl});const session=await other.connectNetwork(),access=peerNetwork(session),signal=new AbortController().signal;
 const peers=await access.query(id,signal),partial=peers.filter(peer=>!peer.complete);assert(partial.length===1,'partial holding must not advertise complete');
 connection=await access.dial(partial[0]!,Uint8Array.from(id.slice(7).match(/../g)!,v=>parseInt(v,16)),signal);
 await connection.rtc.sendControl(wasm.encode_peer_bitfield(new Uint8Array(1),4),signal);
 available=wasm.peer_bitfield(await connection.rtc.readControl(signal),4);assert(available[0]===(recovered?1:0),'committed availability mismatch');
 if(recovered)return;
 await connection.rtc.sendControl(JSON.stringify({v:1,message:{type:'request',block_index:0,count:1}}),signal);
 const rejected=JSON.parse(wasm.decode_peer_control(await connection.rtc.readControl(signal)));assert(rejected.type==='reject');
}
export async function getCommitted(seedBytes:number[],existing=false){
 const wasm=await initializeWasm(),signal=new AbortController().signal;
 if(!existing){const message=JSON.parse(wasm.decode_peer_control(await connection.rtc.readControl(AbortSignal.timeout(10000))));assert(message.type==='have'&&message.block_index==='0');}
 await connection.rtc.sendControl(JSON.stringify({v:1,message:{type:'request',block_index:0,count:1}}),signal);
 const proof=await connection.rtc.readControl(AbortSignal.timeout(10000));
 const assembly=new wasm.PeerBlockAssembly(262144),verifier=new wasm.PeerProofVerifier(new Uint8Array(seedBytes));
 try{
  while(!assembly.complete()){const piece=await connection.rtc.readPiece(AbortSignal.timeout(10000));assert(piece.blockIndex===0n);assembly.insert(piece.offset,piece.data);}
  const bytes=assembly.bytes(),witness=verifier.witness(0,bytes,proof);assert(verifier.verify(0,bytes,witness));
  for(let i=0;i<bytes.length;i++)assert(bytes[i]===(i*31)%251);
 }finally{assembly.free();verifier.free();}
 await connection.rtc.sendControl(JSON.stringify({v:1,message:{type:'block_done',block_index:0}}),signal);
 await connection.rtc.sendControl(JSON.stringify({v:1,message:{type:'request',block_index:1,count:1}}),signal);
 const rejected=JSON.parse(wasm.decode_peer_control(await connection.rtc.readControl(signal)));assert(rejected.type==='reject','missing block must not be served');
}
export async function closeProbe(){await connection?.close();await other?.close();}

export async function rejectedCommit(){try{await transfer.wait();throw Error('commit unexpectedly succeeded');}catch(error){assert((error as Error).message!=='commit unexpectedly succeeded');}assert(publication.snapshot().state==='closed');}
export async function pause(){await transfer.pause();assert(publication.snapshot().state==='closed');assert(transfer.snapshot().state==='paused');}
export async function stopSharing(){await publication.close();assert(!['cancelled','failed'].includes(transfer.snapshot().state));}

export async function corrupt(){corruptBytes();}
export async function sharingFailed(){return publication.snapshot().state;}
export async function consumeFailure(seedBytes:number[]){
 try{await getCommitted(seedBytes);return false;}catch{return true;}
}

export async function restartShared(){
 await transfer.pause();const checkpointId=transfer.snapshot().checkpointId!;await transfer.cancel();
 const handles=client.resumeDownloadShared({...restartOptions,checkpointId});transfer=handles.transfer;publication=handles.publication;
 await publication.ready();assert(transfer.snapshot().committedBytes>=262144n);
}
