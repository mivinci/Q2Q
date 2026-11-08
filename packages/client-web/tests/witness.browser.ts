import {SeedBuilder} from '@q2q/wasm';
import {initializeWasm,createPeerBlockVerifier} from '../src/block-verifier.ts';
import {browserState,opfsCache,resumeCheckpoint,StorageError} from '../src/index.ts';
import type {CheckpointSession} from '../src/index.ts';
const BLOCK=262144;
function assert(value:unknown,message='assertion failed'):asserts value{if(!value)throw Error(message);}
async function rejects(work:Promise<unknown>,code:string){try{await work;}catch(value){assert(value instanceof StorageError&&value.code===code,String(value));return;}throw Error(`expected ${code}`);}
async function material(){
  await initializeWasm();const bytes=new Uint8Array(2*BLOCK+17);for(let i=0;i<bytes.length;i++)bytes[i]=(i*31+Math.floor(i/BLOCK))%251;
  const builder=new SeedBuilder(bytes.length);builder.update(bytes);const result=builder.finish('witness.bin');const seed=result.seed_bytes(),table=result.proof_bytes();result.free();
  const verifier=await createPeerBlockVerifier({seed});
  const hash=async(prefix:number,...parts:Uint8Array[])=>{const value=new Uint8Array(1+parts.reduce((n,p)=>n+p.length,0));value[0]=prefix;let offset=1;for(const part of parts){value.set(part,offset);offset+=part.length;}return new Uint8Array(await crypto.subtle.digest('SHA-256',value));};
  const blocks=Array.from({length:3},(_,i)=>bytes.subarray(i*BLOCK,Math.min(bytes.length,(i+1)*BLOCK)));
  const layers=[await Promise.all(blocks.map(block=>hash(0,block)))];
  while(layers.at(-1)!.length>1){const last=layers.at(-1)!,next=[];for(let i=0;i<last.length;i+=2)next.push(i+1<last.length?await hash(1,last[i]!,last[i+1]!):last[i]!);layers.push(next);}
  const witnesses=blocks.map((block,index)=>{
    const nodes=[];for(let level=0,pos=index;level<layers.length-1;level++,pos=Math.floor(pos/2)){const sibling=pos^1;if(sibling<layers[level]!.length)nodes.push({level,index:sibling,hash:Array.from(layers[level]![sibling]!)});}
    return verifier.verifyPeer!(BigInt(index),block,JSON.stringify({v:1,message:{type:'proof',block_index:index,count:1,proof:{leaf_hashes:[Array.from(layers[0]![index]!)],nodes}}}));
  });return {bytes,blocks,seed,table,verifier,witnesses};
}
async function fixture(run:(f:Awaited<ReturnType<typeof setup>>)=>Promise<void>){
  const f=await setup();try{await run(f);}finally{for(const session of f.sessions)await session.abort().catch(()=>{});await f.state.close();await f.cache.remove(f.entry.id);f.verifier.close();}
}
async function setup(){
  const f=await material(),namespace=crypto.randomUUID(),state=await browserState({namespace}),cache=await opfsCache();
  const entry=await cache.create({name:'witness.bin',size:f.verifier.content.size});const record=await state.create({content:f.verifier.content,targetId:entry.targetId});
  const sessions:CheckpointSession[]=[];
  return {...f,namespace,state,cache,entry,record,sessions,async open(){const session=await resumeCheckpoint({state,checkpointId:record.id,verifier:f.verifier,destination:entry.destination});sessions.push(session);return session;}};
}
async function mutate(namespace:string,id:string,index:number,fn:(store:IDBObjectStore,key:IDBValidKey)=>void){
  const db=await new Promise<IDBDatabase>((resolve,reject)=>{const request=indexedDB.open('q2q-checkpoints-v1');request.onsuccess=()=>resolve(request.result);request.onerror=()=>reject(request.error);});
  try{await new Promise<void>((resolve,reject)=>{const tx=db.transaction('witnesses','readwrite');tx.oncomplete=()=>resolve();tx.onabort=()=>reject(tx.error);fn(tx.objectStore('witnesses'),[namespace,id,index]);});}finally{db.close();}
}
export const cases:Record<string,()=>Promise<void>>={
  'legacy state adapters keep dense recovery and explicitly reject sparse writes':async()=>fixture(async f=>{
    let opens=0;
    const destination={...f.entry.destination,async open(options:Parameters<typeof f.entry.destination.open>[0]){opens++;return f.entry.destination.open(options);}};
    const state={...f.state,async lease(id:string){const lease=await f.state.lease(id);return {get closed(){return lease.closed;},signal:lease.signal,snapshot:()=>lease.snapshot(),replace:(revision:number,bits:Uint8Array)=>lease.replace(revision,bits),close:()=>lease.close()};}};
    await rejects(resumeCheckpoint({state,checkpointId:f.record.id,verifier:f.verifier,destination}),'UNSUPPORTED_CAPABILITY');assert(opens===0);
    const {createBlockVerifier}=await import('../src/block-verifier.ts');const dense=await createBlockVerifier({seed:f.seed,proof:f.table});
    try{const session=await resumeCheckpoint({state,checkpointId:f.record.id,verifier:dense,destination});try{await session.writeBlock(0n,f.blocks[0]!);await session.commit();}finally{await session.abort();}assert(Number(opens)===1);}finally{dense.close();}
  }),
  'witness metadata is copied, isolated, CAS-bound and discarded':async()=>fixture(async f=>{
    const lease=await f.state.lease(f.record.id);const input=f.witnesses[0]!.slice();
    await lease.replace(1,new Uint8Array([1]),new Map([[0,input]]));input[0]^=1;
    const stored=await lease.witness!(0);assert(stored&&stored[0]!==input[0]);stored[0]^=1;assert((await lease.witness!(0))![0]!==stored[0]);
    await rejects(lease.replace(1,new Uint8Array([3]),new Map([[1,f.witnesses[1]!]])),'CHECKPOINT_CONFLICT');assert(await lease.witness!(1)===null);
    await rejects(lease.replace(2,new Uint8Array([1]),new Map([[1,f.witnesses[1]!]])),'INVALID_RANGE');
    const other=await browserState({namespace:`other-${crypto.randomUUID()}`});try{await rejects(other.read(f.record.id),'CHECKPOINT_NOT_FOUND');}finally{await other.close();}
    await lease.close();await f.state.discard(f.record.id);
    let found=false;await mutate(f.namespace,f.record.id,0,(store,key)=>{const get=store.get(key);get.onsuccess=()=>{found=!!get.result;};});assert(!found);
  }),
  'proof and bitmap abort together when witness quota fails after target commit':async()=>fixture(async f=>{
    const session=await f.open();await session.writeBlock(0n,f.blocks[0]!,f.witnesses[0]);await session.writeBlock(1n,f.blocks[1]!,f.witnesses[1]);
    const put=IDBObjectStore.prototype.put;
    IDBObjectStore.prototype.put=function(value,...rest){if(this.name==='witnesses'&&value.index===1)throw new DOMException('injected','QuotaExceededError');return put.call(this,value,...rest);};
    try{await rejects(session.commit(),'STORAGE_FULL');}finally{IDBObjectStore.prototype.put=put;}
    assert((await f.state.read(f.record.id)).committedBytes===0n);const lease=await f.state.lease(f.record.id);
    try{assert(await lease.witness!(0)===null&&await lease.witness!(1)===null);}finally{await lease.close();}
  }),
  'swapped witnesses and changed bytes cannot register committed blocks':async()=>fixture(async f=>{
    const session=await f.open();await rejects(session.writeBlock(1n,f.blocks[1]!,f.witnesses[0]),'CONTENT_MISMATCH');
    const wrong=f.blocks[0]!.slice();wrong[0]^=1;await rejects(session.writeBlock(0n,wrong,f.witnesses[0]),'CONTENT_MISMATCH');
    await session.writeBlock(0n,f.blocks[0]!,f.witnesses[0]);await session.commit();assert((await f.state.read(f.record.id)).committedBytes===BigInt(BLOCK));
  }),
  'missing or corrupted witnesses are withdrawn before recovery':async()=>fixture(async f=>{
    let session=await f.open();await session.writeBlock(0n,f.blocks[0]!,f.witnesses[0]);await session.commit();await session.abort();
    await mutate(f.namespace,f.record.id,0,(store,key)=>{const get=store.get(key);get.onsuccess=()=>{const row=get.result;row.bytes[0]^=1;store.put(row);};});
    session=await f.open();assert(session.snapshot().committedBytes===0n);await session.writeBlock(0n,f.blocks[0]!,f.witnesses[0]);await session.commit();await session.abort();
    await mutate(f.namespace,f.record.id,0,(store,key)=>{store.delete(key);});session=await f.open();assert(session.snapshot().committedBytes===0n);
  }),
  'future witness versions stop recovery without downgrading metadata':async()=>fixture(async f=>{
    const session=await f.open();await session.writeBlock(0n,f.blocks[0]!,f.witnesses[0]);await session.commit();await session.abort();
    await mutate(f.namespace,f.record.id,0,(store,key)=>{const get=store.get(key);get.onsuccess=()=>store.put({...get.result,version:99});});
    await rejects(f.open(),'CHECKPOINT_VERSION');assert((await f.state.read(f.record.id)).committedBytes===BigInt(BLOCK));
  }),
  'version-one databases upgrade in place and retain old checkpoints':async()=>{
    const f=await material();const namespace=crypto.randomUUID(),id=crypto.randomUUID();
    const db=await new Promise<IDBDatabase>((resolve,reject)=>{const request=indexedDB.open('q2q-checkpoints-v1',1);request.onupgradeneeded=()=>request.result.createObjectStore('checkpoints',{keyPath:['namespace','id']});request.onsuccess=()=>resolve(request.result);request.onerror=()=>reject(request.error);});
    await new Promise<void>((resolve,reject)=>{const tx=db.transaction('checkpoints','readwrite');tx.objectStore('checkpoints').put({namespace,id,version:1,revision:1,contentId:f.verifier.content.id,size:f.verifier.content.size.toString(),blockSize:BLOCK,targetId:'old-target',committed:new Uint8Array([1]),updatedAt:Date.now()});tx.oncomplete=()=>resolve();tx.onabort=()=>reject(tx.error);});db.close();
    const state=await browserState({namespace});try{assert((await state.read(id)).revision===1);const lease=await state.lease(id);try{assert(await lease.witness!(0)===null);}finally{await lease.close();}}finally{await state.close();f.verifier.close();}
  }
};
