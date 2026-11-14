import { createClient,opfsCache,browserState,Q2QError } from '../src/index.ts';
import type { Client, Publication } from '../src/index.ts';
function assert(value:unknown,message='assertion failed'):asserts value{if(!value)throw Error(message);}
async function digest(bytes:Uint8Array){const copy=bytes.slice();return Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',copy.buffer))).map(value=>value.toString(16).padStart(2,'0')).join('');}
function closeError(error:unknown){return error instanceof Q2QError?error.code:'not-q2q-error';}
async function promiseError(work:Promise<unknown>){try{await work;return undefined;}catch(error){return error;}}

async function exportAll(id:string){
  const cache=await opfsCache();const entry=await cache.open(id);
  const reader=await entry.source.open();const chunks:Uint8Array[]=[];
  try{for(let offset=0n;offset<entry.size;offset+=1048576n){const length=Number(entry.size-offset<1048576n?entry.size-offset:1048576n);chunks.push(new Uint8Array(await reader.read(offset,length,{})));}}
  finally{await reader.close();}
  const bytes=new Uint8Array(chunks.reduce((total,chunk)=>total+chunk.length,0));let offset=0;for(const chunk of chunks){bytes.set(chunk,offset);offset+=chunk.length;}
  return bytes;
}

/** Parse the fixture seed and derive its magnet link with a local-only client. */
export async function linkFromSeed(fixture:{seed:string;trackUrl:string}){
  const client=createClient({trackUrl:fixture.trackUrl});
  try{
    const seed=await client.parseSeed(Uint8Array.from(atob(fixture.seed),c=>c.charCodeAt(0)));
    const link=await client.seedMagnet(seed);
    assert(link.startsWith('magnet:?xt=urn:sha256:'),'link format');
    return {link,infoHash:seed.info.id,size:seed.info.size};
  }finally{await client.close();}
}

/** Native publisher serves metadata: resolve from a fresh identity, download,
 * and compare the full content hash against an independent digest. */
export async function resolveAndDownload(fixture:{seed:string;trackUrl:string}){
  const {link,infoHash,size}=await linkFromSeed(fixture);
  const state=await browserState({namespace:crypto.randomUUID()});
  const client=createClient({trackUrl:fixture.trackUrl,state});
  const cache=await opfsCache();
  try{
    const resolved=await client.resolveMagnet(link);
    assert(resolved.info.id===infoHash&&resolved.info.size===size,'resolved identity mismatch');
    assert(resolved.hints.contentSources===0&&resolved.hints.proofSources===0,'minimal seed must carry no HTTP sources');
    const entry=await cache.create({name:'magnet.bin',size});
    const task=client.download({seed:resolved,destination:entry.destination});
    await task.wait();
    assert(task.snapshot().state==='completed','download completed');
    const bytes=await exportAll(entry.id);
    assert(bytes.length===Number(size),'downloaded size mismatch');
    return {infoHash,hash:await digest(bytes)};
  }finally{await client.close();await state.close();}
}

/** Cache-first resolution works with the publisher already gone. */
export async function resolveFromCache(trackUrl:string,link:string){
  const state=await browserState({namespace:crypto.randomUUID()});
  const client=createClient({trackUrl,state});
  try{
    const resolved=await client.resolveMagnet(link);
    assert(typeof resolved.info.id==='string'&&resolved.info.id.startsWith('sha256:'),'cached resolve');
    return resolved.info.id;
  }finally{await client.close();await state.close();}
}

/** Failure taxonomy: invalid links never reach the network. */
export async function parseFailures(trackUrl:string){
  const client=createClient({trackUrl});
  try{
    const invalid=['','not a magnet','magnet:?dn=name',`magnet:?xt=urn:sha256:${'z'.repeat(64)}`,`magnet:?xt=urn:sha256:${'a'.repeat(63)}`];
    for(const link of invalid){
      const error=await promiseError(client.parseMagnet(link));
      assert(error instanceof Q2QError&&error.code==='INVALID_MAGNET',`${link}: ${closeError(error)}`);
    }
    for(const namespace of ['btih','btmh']){
      const error=await promiseError(client.parseMagnet(`magnet:?xt=urn:${namespace}:0123456789abcdef0123456789abcdef01234567`));
      assert(error instanceof Q2QError&&error.code==='MAGNET_UNSUPPORTED',`${namespace}: ${closeError(error)}`);
    }
    const parsed=await client.parseMagnet(`magnet:?xt=urn:sha256:${'ab'.repeat(32)}&dn=x&tr=track.example.com`);
    assert(parsed.infoHash==='ab'.repeat(32)&&parsed.displayName==='x','parsed fields');
    assert(parsed.trackers.length===1&&parsed.trackers[0]==='https://track.example.com','tr normalization');
  }finally{await client.close();}
}

/** Resolution without a Track, unknown hashes and cancellation stay typed. */
export async function resolveFailures(trackUrl:string){
  const offline=createClient({});
  try{
    const error=await promiseError(offline.resolveMagnet(`magnet:?xt=urn:sha256:${'cd'.repeat(32)}`));
    assert(error instanceof Q2QError&&error.code==='UNSUPPORTED_CAPABILITY',closeError(error));
  }finally{await offline.close();}

  const state=await browserState({namespace:crypto.randomUUID()});
  const client=createClient({trackUrl,state});
  try{
    const error=await promiseError(client.resolveMagnet(`magnet:?xt=urn:sha256:${'cd'.repeat(32)}`));
    assert(error instanceof Q2QError&&error.code==='NO_SOURCES',closeError(error));
    const controller=new AbortController();
    const work=client.resolveMagnet(`magnet:?xt=urn:sha256:${'ef'.repeat(32)}`,{signal:controller.signal});
    controller.abort();
    const cancelled=await promiseError(work);
    assert(cancelled instanceof Q2QError&&cancelled.code==='TASK_CANCELLED',closeError(cancelled));
  }finally{await client.close();await state.close();}
}

/** Corrupt cache records degrade to rediscovery and heal on success. */
export async function corruptCache(fixture:{seed:string;trackUrl:string}){
  const {link,infoHash}=await linkFromSeed(fixture);
  const raw=`sha256:${infoHash.slice(7)}`;
  await writeCacheRecord(raw,new TextEncoder().encode('corrupt metadata bytes'));
  const state=await browserState({namespace:crypto.randomUUID()});
  const client=createClient({trackUrl:fixture.trackUrl,state});
  try{
    const resolved=await client.resolveMagnet(link);
    assert(resolved.info.id===infoHash,'corrupt cache did not fall back to network');
    const record=await readCacheRecord(raw);
    assert(record!==null&&new TextDecoder().decode(record)!=='corrupt metadata bytes','cache not rewritten');
    const again=await client.resolveMagnet(link);
    assert(again.info.id===infoHash,'healed cache mismatch');
  }finally{await client.close();await state.close();}
  await clearCacheDatabase();
}

async function openCacheStore(mode:IDBTransactionMode){
  const db=await new Promise<IDBDatabase>((resolve,reject)=>{const request=indexedDB.open('q2q-magnet-v1',1);request.onupgradeneeded=()=>{if(!request.result.objectStoreNames.contains('magnets'))request.result.createObjectStore('magnets',{keyPath:'hash'});};request.onsuccess=()=>resolve(request.result);request.onerror=()=>reject(request.error);});
  return new Promise<{store:IDBObjectStore;done:Promise<void>}>((resolve,reject)=>{
    const tx=db.transaction('magnets',mode);
    const done=new Promise<void>((complete,fail)=>{tx.oncomplete=()=>complete();tx.onerror=()=>fail(tx.error);tx.onabort=()=>fail(tx.error);});
    void done.finally(()=>db.close()).catch(()=>{});
    resolve({store:tx.objectStore('magnets'),done});
  });
}
async function writeCacheRecord(hash:string,bytes:Uint8Array){const {store,done}=await openCacheStore('readwrite');store.put({hash,bytes,writtenAt:Date.now()});await done;}
async function readCacheRecord(hash:string){const {store,done}=await openCacheStore('readonly');const request=store.get(hash);await done;return (request.result as {bytes?:Uint8Array}|undefined)?.bytes??null;}
async function clearCacheDatabase(){await new Promise<void>(resolve=>{const request=indexedDB.deleteDatabase('q2q-magnet-v1');request.onsuccess=()=>resolve();request.onerror=()=>resolve();request.onblocked=()=>resolve();});}

/** A peer whose info does not match the link is rejected, not trusted. */
export async function badMetadataPeer(fixture:{seed:string;trackUrl:string}){
  const {RtcPeer}=await import('../src/rtc-peer.ts');
  const readControl=RtcPeer.prototype.readControl;
  let tampered=false;
  RtcPeer.prototype.readControl=async function(signal:AbortSignal){
    const raw=await readControl.call(this,signal);
    if(!tampered&&JSON.parse(raw).message?.type==='info'){
      tampered=true;
      const value=JSON.parse(raw) as {message:{data:string}};
      // Flip the first base64 character: same length, different bytes, wrong hash.
      value.message.data=(value.message.data[0]==='A'?'B':'A')+value.message.data.slice(1);
      return JSON.stringify(value);
    }
    return raw;
  };
  try{
    const {link,infoHash}=await linkFromSeed(fixture);
    const state=await browserState({namespace:crypto.randomUUID()});
    const client=createClient({trackUrl:fixture.trackUrl,state});
    try{
      const error=await promiseError(client.resolveMagnet(link));
      assert(error instanceof Q2QError&&error.code==='METADATA_MISMATCH',`${closeError(error)} for ${infoHash}`);
    }finally{await client.close();await state.close();}
  }finally{RtcPeer.prototype.readControl=readControl;}
}

/** Web publisher half: publish from an in-memory source, expose the magnet
 * link, keep serving until close. Runs in its own page: two RTCPeerConnections
 * inside one page never complete ICE, so the receiver must be a second page. */
export async function webPublishForMagnet(trackUrl:string,content:Uint8Array){
  const source={info:{name:'web-magnet.bin',size:BigInt(content.length)},
    async open(){return{async read(offset:bigint,length:number){return content.slice(Number(offset),Number(offset)+length);},async close(){}};}};
  const state=await browserState({namespace:crypto.randomUUID()});
  const client=createClient({trackUrl,state});
  const publication=publisherOf(client,source);
  window.__magnet={client,state,publication};
  const ready=await publication.ready();
  const link=await client.seedMagnet(ready.seed);
  assert(link.startsWith('magnet:?xt=urn:sha256:'),'web link format');
  return {link,infoId:ready.seed.info.id,size:ready.seed.info.size};
}

/** Stop the web publication and release its page resources. */
export async function webPublishClose(){
  const held=window.__magnet;
  if(!held)return;
  window.__magnet=undefined;
  await held.publication.close().catch(()=>{});
  await held.client.close().catch(()=>{});
  await held.state.close().catch(()=>{});
}

declare global {
  interface Window {
    __magnet?:{client:Client;state:Awaited<ReturnType<typeof browserState>>;publication:Publication};
  }
}

function publisherOf(client:Client,source:{info:{name:string;size:bigint};open():Promise<{read(offset:bigint,length:number):Promise<Uint8Array>;close():Promise<void>}>}){
  return client.publish({source});
}

/** Web receiver half, in a second page: resolve from the other page's link,
 * download, and verify the content hash. An in-memory destination keeps this
 * independent of WebKit private-profile OPFS limits, mirroring the
 * publication suite; persistent OPFS paths have their own suites. */
export async function webResolveAndDownload(trackUrl:string,link:string,expected:{infoId:string;size:bigint;hash:string}){
  const receiver=createClient({trackUrl});
  try{
    const resolved=await receiver.resolveMagnet(link);
    assert(resolved.info.id===expected.infoId,'web metadata identity mismatch');
    assert(resolved.info.size===expected.size,'web metadata size');
    const actual=new Uint8Array(Number(expected.size)),targetId=crypto.randomUUID();
    const destination={targetId,async open(){return{
      async read(offset:bigint,length:number){return actual.slice(Number(offset),Number(offset)+length);},
      async write(offset:bigint,chunk:Uint8Array){actual.set(chunk,Number(offset));},async commit(){},
      async finish(){return {kind:'staged' as const,targetId,needsExport:true as const};},async abort(){}
    };}};
    const task=receiver.download({seed:resolved,destination});
    await task.wait();
    assert(task.snapshot().state==='completed','download completed');
    assert(await digest(actual)===expected.hash,'web-to-web content hash mismatch');
    // A metadata-only connection closed cleanly; resolution still works.
    const second=await receiver.resolveMagnet(link);
    assert(second.info.id===expected.infoId,'post-download resolve mismatch');
    return true;
  }finally{await receiver.close().catch(()=>{});}
}
