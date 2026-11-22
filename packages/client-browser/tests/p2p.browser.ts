import { createClient,opfsCache,browserState,Q2QError } from '../src/index.ts';
import { RtcPeer } from '../src/rtc-peer.ts';
import type { Transfer } from '../src/index.ts';
function assert(value:unknown,message='assertion failed'):asserts value{if(!value)throw Error(message);}
export async function download(fixture:{seed:string;trackUrl:string;heartbeatMs?:number},pause:boolean,fault?:'drop'|'corrupt'|'fallback'|'cancel'|'external'|'parallel'|'proof'|'sparse'|'sparse-corrupt'|'sparse-proof-corrupt'|'sparse-memory'){
  const readControl=RtcPeer.prototype.readControl,readPiece=RtcPeer.prototype.readPiece,sendControl=RtcPeer.prototype.sendControl;let injected=false;let repairs=0;let proofs=0;
  RtcPeer.prototype.readControl=async function(signal){
    for(;;){const raw=await readControl.call(this,signal);if(fault==='proof'&&!injected&&JSON.parse(raw).message.type==='proof'){injected=true;continue;}if(fault==='sparse-proof-corrupt'&&!injected&&JSON.parse(raw).message.type==='proof'){injected=true;const value=JSON.parse(raw);value.message.proof.nodes[0].hash[0]^=1;return JSON.stringify(value);}return raw;}
  };
  RtcPeer.prototype.readPiece=async function(signal){
    for(;;){const piece=await readPiece.call(this,signal);
      if(!injected&&piece.blockIndex===0n&&piece.offset===1024&&fault&&['drop','corrupt','fallback','sparse-corrupt'].includes(fault)){
        injected=true;if(fault==='drop')continue;const data=piece.data.slice();data[0]^=1;return {...piece,data};
      }return piece;
    }
  };
  RtcPeer.prototype.sendControl=function(raw,signal){if(JSON.parse(raw).message.type==='piece_nack')repairs++;if(JSON.parse(raw).message.type==='request_proof')proofs++;return sendControl.call(this,raw,signal);};
  const state=await browserState({namespace:crypto.randomUUID()});
  const client=createClient({trackUrl:fixture.trackUrl,...(fault==='sparse-memory'?{}:{state}),...(fault==='external'?{sourceProvider:{async resolve(){return {contentUrls:[],proofUrls:[`${location.origin}/p2p-proof`]};}}}:{}),...(fault==='fallback'?{sourceProvider:{async resolve(){return {contentUrls:[`${location.origin}/p2p-content`]};}}}:{}),limits:{peerConnections:1,commitBytes:16777216}});
  const cache=await opfsCache();const seed=await client.parseSeed(Uint8Array.from(atob(fixture.seed),c=>c.charCodeAt(0)));
  assert(seed.hints.contentSources===0&&seed.hints.proofSources===0);assert(client.capabilities.p2p);
  let entry=await cache.create({name:'p2p.bin',size:seed.info.size});const entries=[entry];let task:Transfer|undefined;
  try{
    const session=await client.connectNetwork();
    const heartbeat=setInterval(()=>void session.heartbeat().catch(()=>{}),fixture.heartbeatMs??200);
    try{
      task=client.download({seed,destination:entry.destination});
      if(fault==='cancel'||fault==='parallel'){
        const until=performance.now()+30000;
        while(task.snapshot().receivedBytes===0n&&task.snapshot().state!=='failed'&&performance.now()<until)await new Promise(resolve=>setTimeout(resolve,10));
        assert(task.snapshot().receivedBytes>0n);
        let queued:Transfer|undefined;
        if(fault==='parallel'){entry=await cache.create({name:'parallel.bin',size:seed.info.size});entries.push(entry);queued=client.download({seed,destination:entry.destination});}
        await task.cancel();assert(task.snapshot().state==='cancelled');
        const checkpointId=task.snapshot().checkpointId!;await session.heartbeat();
        task=queued??client.resumeDownload({seed,destination:entry.destination,checkpointId});
      }
      if(fault==='corrupt'||fault==='sparse-corrupt'||fault==='sparse-proof-corrupt'){
        try{await task.wait();throw Error('corrupt peer completed');}catch(error){assert(error instanceof Q2QError&&error.code==='CONTENT_MISMATCH',String(error));}
        assert(injected&&task.snapshot().committedBytes===0n);const saved=await state.read(task.snapshot().checkpointId!);assert(saved.committed.every(byte=>byte===0));return;
      }
      if(pause){
        const until=performance.now()+30000;
        while(task.snapshot().committedBytes===0n&&task.snapshot().state!=='failed'&&performance.now()<until)await new Promise(resolve=>setTimeout(resolve,10));
        assert(task.snapshot().committedBytes>0n,JSON.stringify(task.snapshot(),(_,v)=>typeof v==='bigint'?v.toString():v));
        await task.pause();assert(task.snapshot().state==='paused');assert(task.snapshot().committedBytes<seed.info.size);
        await session.heartbeat();await task.resume();
      }
      const result=await task.wait();assert(result.output.kind==='staged');assert(task.snapshot().committedBytes===seed.info.size);
      const root=await navigator.storage.getDirectory();
      // Export through the existing destination contract, then independently hash/read bytes.
      const target=await root.getFileHandle(`p2p-export-${crypto.randomUUID()}`,{create:true});
      const {fileDestination}=await import('../src/index.ts');await entry.exportTo(await fileDestination(target));
      const bytes=new Uint8Array(await(await target.getFile()).arrayBuffer());assert(bytes.length===3*262144+17);
      for(let i=0;i<bytes.length;i++)assert(bytes[i]===(i*31+Math.floor(i/262144))%251,`byte ${i}`);
      const hash=Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',bytes)),v=>v.toString(16).padStart(2,'0')).join('');
      assert(hash==='3ba31e034e8297dab9200e0e697a3440048b3d9d113d5460ad0b5cff9c324cd1',hash);await root.removeEntry(target.name);
      assert((await client.listCheckpoints()).items.length===(fault==='parallel'?2:1));
      if(fault==='drop')assert(injected&&repairs>0);
      if(fault==='proof')assert(injected&&proofs>0);
      if(fault==='fallback')assert(injected&&task.snapshot().receivedBytes>seed.info.size);
    }finally{clearInterval(heartbeat);}
  }finally{RtcPeer.prototype.readControl=readControl;RtcPeer.prototype.readPiece=readPiece;RtcPeer.prototype.sendControl=sendControl;await task?.cancel();await client.close();await state.close();for(const item of entries)await cache.remove(item.id);}
}

export async function prepareSparseReload(fixture:{seed:string;trackUrl:string}){
  const namespace=crypto.randomUUID(),state=await browserState({namespace}),client=createClient({trackUrl:fixture.trackUrl,state});
  const cache=await opfsCache(),seed=await client.parseSeed(Uint8Array.from(atob(fixture.seed),c=>c.charCodeAt(0))),entry=await cache.create({name:'reload.bin',size:seed.info.size});
  const task=client.download({seed,destination:entry.destination});
  try{
    const until=performance.now()+30000;while(task.snapshot().committedBytes===0n&&task.snapshot().state!=='failed'&&performance.now()<until)await new Promise(resolve=>setTimeout(resolve,10));
    assert(task.snapshot().committedBytes>0n,String(task.snapshot().error?.code));await task.pause();
    const checkpointId=task.snapshot().checkpointId!,record=await state.read(checkpointId);assert(record.committedBytes<seed.info.size);
    const lease=await state.lease(checkpointId);try{assert((await lease.witness!(0))!.length<=640);}finally{await lease.close();}
    return {...fixture,namespace,entryId:entry.id,checkpointId,committed:record.committedBytes.toString()};
  }finally{await client.close();await state.close();}
}
export async function checkSparseReload(saved:Awaited<ReturnType<typeof prepareSparseReload>>,tamper:'none'|'file'|'proof'|'missing'){
  const state=await browserState({namespace:saved.namespace}),client=createClient({trackUrl:saved.trackUrl,state}),cache=await opfsCache(),entry=await cache.open(saved.entryId);
  const seed=await client.parseSeed(Uint8Array.from(atob(saved.seed),c=>c.charCodeAt(0)));
  if(tamper==='file'){
    const root=await navigator.storage.getDirectory(),directory=await(await root.getDirectoryHandle('q2q-cache-v1')).getDirectoryHandle(saved.entryId);
    const file=await directory.getFileHandle('content'),writer=await file.createWritable({keepExistingData:true});await writer.write(new Uint8Array([255]));await writer.close();
  }
  if(tamper==='proof'||tamper==='missing'){
    const db=await new Promise<IDBDatabase>((resolve,reject)=>{const req=indexedDB.open('q2q-checkpoints-v1');req.onsuccess=()=>resolve(req.result);req.onerror=()=>reject(req.error);});
    try{await new Promise<void>((resolve,reject)=>{const tx=db.transaction('witnesses','readwrite'),store=tx.objectStore('witnesses'),key=[saved.namespace,saved.checkpointId,0];
      if(tamper==='missing')store.delete(key);else{const read=store.get(key);read.onsuccess=()=>{const row=read.result;row.bytes[row.bytes.length-1]^=1;store.put(row);};}
      tx.oncomplete=()=>resolve();tx.onabort=()=>reject(tx.error);
    });}finally{db.close();}
  }
  const send=RtcPeer.prototype.sendControl;const requested=new Set<number>();
  RtcPeer.prototype.sendControl=function(raw,signal){const value=JSON.parse(raw).message;if(value.type==='request')requested.add(value.block_index);return send.call(this,raw,signal);};
  const task=client.resumeDownload({seed,destination:entry.destination,checkpointId:saved.checkpointId});let initial:bigint|undefined;
  const unsubscribe=task.subscribe(snapshot=>{if(initial===undefined&&snapshot.state==='downloading')initial=snapshot.committedBytes;});
  try{
    await task.wait();assert(tamper==='none'?initial===BigInt(saved.committed):initial!<BigInt(saved.committed));
    assert(tamper==='none'?!requested.has(0):requested.has(0));
    assert(seed.info.size===786449n,'fixture content size must stay bounded');
    const recovered=new Uint8Array(Number(seed.info.size));
    const reader=await entry.source.open();try{for(let i=0;i<Number(seed.info.size);i+=65536){const bytes=await reader.read(BigInt(i),Math.min(65536,Number(seed.info.size)-i));recovered.set(bytes,i);for(let j=0;j<bytes.length;j++)assert(bytes[j]===((i+j)*31+Math.floor((i+j)/262144))%251,`recovered byte ${i+j}`);}}finally{await reader.close();}
    const digest=Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',recovered)),value=>value.toString(16).padStart(2,'0')).join('');
    assert(digest==='3ba31e034e8297dab9200e0e697a3440048b3d9d113d5460ad0b5cff9c324cd1',digest);
  }finally{unsubscribe();RtcPeer.prototype.sendControl=send;await task.cancel();await client.close();await state.discard(saved.checkpointId);await state.close();await cache.remove(entry.id);}
}
