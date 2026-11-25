import {createClient,fileSource} from '../src/index.ts';
import type {Client,Publication} from '../src/index.ts';
let client:Client|undefined,publication:Publication|undefined;let mutate:(()=>void)|undefined;
const check=(value:unknown,message='publication assertion failed')=>{if(!value)throw Error(message);};
export async function start(trackUrl:string,mutable=false){
 const bytes=new Uint8Array(3*262144+17);for(let i=0;i<bytes.length;i++)bytes[i]=(i*31+Math.floor(i/262144))%251;
 client=createClient({trackUrl});check(client.capabilities.publish);
 let changed=false;mutate=()=>{changed=true;};
 const source=mutable?{info:{name:'published.bin',size:BigInt(bytes.length)},async open(){return{async read(offset:bigint,length:number){const result=bytes.slice(Number(offset),Number(offset)+length);if(changed&&result.length)result[0]!^=1;return result;},async close(){}};}}:fileSource(new File([bytes],'published.bin'));
 publication=client.publish({source});
 const value=await publication.ready();check(publication.snapshot().state==='publishing');
 return {seed:Array.from(value.seed.toBytes()),id:value.seed.info.id};
}
export async function change(){mutate!();}
export async function state(){return publication!.snapshot().state;}
export async function pause(){await publication!.pause();check(publication!.snapshot().state==='paused');}
export async function resume(){await publication!.resume();check(publication!.snapshot().state==='publishing');}
export async function close(){await publication?.close();await client?.close();}
export async function receive(trackUrl:string,bytes:number[],drop=false){
 const client=createClient({trackUrl});const seed=await client.parseSeed(new Uint8Array(bytes));
 check(seed.info.size===786449n);const actual=new Uint8Array(Number(seed.info.size)),targetId=crypto.randomUUID();
 // Publication tests isolate peer serving from WebKit private-profile OPFS limits.
 // Production OPFS and file persistence have their own persistent-profile suite.
 const destination={targetId,async open(){return{
  async read(offset:bigint,length:number){return actual.slice(Number(offset),Number(offset)+length);},
  async write(offset:bigint,chunk:Uint8Array){actual.set(chunk,Number(offset));},async commit(){},
  async finish(){return {kind:'staged' as const,targetId,needsExport:true as const};},async abort(){}
 };}};
 const {RtcPeer}=await import('../src/rtc-peer.ts');const original=RtcPeer.prototype.readPiece;let dropped=false;
 if(drop)RtcPeer.prototype.readPiece=async function(signal){for(;;){const piece=await original.call(this,signal);if(!dropped&&piece.offset===1024){dropped=true;continue;}return piece;}};
 const task=client.download({seed,destination});
 try{
  await task.wait();
  const digest=Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',actual)),value=>value.toString(16).padStart(2,'0')).join('');
  check(digest==='3ba31e034e8297dab9200e0e697a3440048b3d9d113d5460ad0b5cff9c324cd1',digest);if(drop)check(dropped);
 }finally{RtcPeer.prototype.readPiece=original;await task.cancel();await client.close();}

}

export async function checkLifecycle(trackUrl:string){
 const client=createClient({trackUrl});let closeCount=0;let release!:()=>void,entered!:()=>void;
 const gate=new Promise<void>(resolve=>{release=resolve;});
 const opened=new Promise<void>(resolve=>{entered=resolve;});
 const source={info:{name:'blocked.bin',size:1n},async open(){entered();await gate;return{async read(){return new Uint8Array([1]);},async close(){closeCount++;}};}};
 const publication=client.publish({source});let notifications=0;const unsubscribe=publication.subscribe(()=>{notifications++;throw Error('isolated observer');});
 const ready=publication.ready().then(()=>false,error=>error.code==='TASK_CANCELLED');
 // Wasm loading may take longer on CI; close only once source acquisition is in flight.
 await opened;const closing=publication.close();
 check(closing===publication.close());release();await closing;check(await ready);check(closeCount===1);check(publication.snapshot().state==='closed');
 unsubscribe();check(notifications>=2);await client.close();
}
export async function checkPauseDuringPreparation(trackUrl:string){
 const client=createClient({trackUrl});let opening=0,closed=0;let release!:()=>void,entered!:()=>void;
 const gate=new Promise<void>(resolve=>{release=resolve;});
 const opened=new Promise<void>(resolve=>{entered=resolve;});
 const source={info:{name:'pause.bin',size:1n},async open(){if(++opening===1){entered();await gate;}return{async read(){return new Uint8Array([7]);},async close(){closed++;}};}};
 const publication=client.publish({source});await opened;
 const pausing=publication.pause();release();await pausing;check(publication.snapshot().state==='paused');
 await publication.resume();await publication.ready();check(publication.snapshot().state==='publishing');
 await client.close();check(publication.snapshot().state==='closed');check(closed===2);
}
export async function listing(trackUrl:string,id:string){
 const own=createClient({trackUrl});try{const session=await own.connectNetwork();const {peerNetwork}=await import('../src/network.ts');return(await peerNetwork(session).query(id,new AbortController().signal)).length;}finally{await own.close();}
}

export async function checkEmptyAndReentrantClose(trackUrl:string){
 const client=createClient({trackUrl});const empty=client.publish({source:fileSource(new File([],'empty.bin'))});
 const first=empty.ready(),second=empty.ready();check(first===second);const ready=await first;check(ready.seed.info.size===0n);
 let nested:Promise<void>|undefined;empty.subscribe(value=>{if(value.state==='closing')nested=empty.close();});
 const closing=empty.close();check(nested===closing);await closing;check(empty.snapshot().state==='closed');
 const replacement=client.publish({source:fileSource(new File([new Uint8Array([1])],'replacement.bin'))});await replacement.ready();
 replacement.subscribe(value=>{if(value.state==='closing')void client.close();});await client.close();check(replacement.snapshot().state==='closed');
}
