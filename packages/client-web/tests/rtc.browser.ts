import { RtcPeer } from '../src/rtc-peer.ts';
import { controlIdentity, decodeKey } from '../src/control-identity.ts';
import { Q2QError } from '../src/error.ts';
function assert(value:unknown,message='assertion failed'):asserts value{if(!value)throw Error(message);}
async function rejects(work:Promise<unknown>,codes:string[]){try{await work;}catch(error){assert(error instanceof Q2QError&&codes.includes(error.code),String(error));return;}throw Error('expected rejection');}
export async function pair(options:{mismatch?:'hash'|'key'|'connection';cancel?:boolean;overflow?:boolean;pressure?:boolean;smallMessageSize?:boolean}={}){
  const one=await controlIdentity(),two=await controlIdentity();const controller=new AbortController();
  let a:RtcPeer|undefined,b:RtcPeer|undefined;const earlyA:RTCIceCandidateInit[]=[],earlyB:RTCIceCandidateInit[]=[];
  const candidateErrors:unknown[]=[];
  const hash=new Uint8Array(32).fill(3);
  try{
    a=await RtcPeer.create({identity:one,remoteKey:decodeKey(two.info.public_key),infoHash:hash,connectionId:'rtc-test',initiator:true,signal:controller.signal,
      onCandidate:c=>{if(b)void b.addCandidate(c).catch(e=>candidateErrors.push(e));else earlyB.push(c);}});
    b=await RtcPeer.create({identity:two,remoteKey:options.mismatch==='key'?new Uint8Array(32):decodeKey(one.info.public_key),
      infoHash:options.mismatch==='hash'?new Uint8Array(32).fill(4):hash,connectionId:options.mismatch==='connection'?'wrong-connection':'rtc-test',initiator:false,signal:controller.signal,
      onCandidate:c=>{if(a)void a.addCandidate(c).catch(e=>candidateErrors.push(e));else earlyA.push(c);}});
    for(const c of earlyA)await a.addCandidate(c);for(const c of earlyB)await b.addCandidate(c);
    if(options.smallMessageSize)Object.defineProperty((a as unknown as {pc:RTCPeerConnection}).pc,'sctp',{get:()=>({maxMessageSize:1024})});
    const offer=await a.offer();
    await rejects(a.sendControl('{"v":1,"message":{"type":"request_info"}}',controller.signal),['INVALID_STATE']);
    if(options.cancel){controller.abort();await rejects(a.ready(),['TASK_CANCELLED']);return;}
    const answer=await b.answer(offer);await a.applyAnswer(answer);
    if(options.smallMessageSize){await rejects(Promise.all([a.ready(),b.ready()]),['UNSUPPORTED_CAPABILITY','PEER_CLOSED']);return;}
    if(options.mismatch){await rejects(Promise.all([a.ready(),b.ready()]),['PEER_AUTH_FAILED','PEER_CLOSED']);return;}
    await Promise.all([a.ready(),b.ready()]);assert(candidateErrors.length===0);
    if(options.pressure){
      const channel=(a as unknown as {data:RTCDataChannel}).data;let blocked=true;
      Object.defineProperty(channel,'bufferedAmount',{configurable:true,get:()=>blocked?256*1024:0});
      const cancelled=new AbortController();
      const jobs=Array.from({length:8},()=>rejects(a!.sendPiece(0n,0,new Uint8Array(1),cancelled.signal),['TASK_CANCELLED']));
      await rejects(a.sendPiece(0n,0,new Uint8Array(1),controller.signal),['RESOURCE_LIMIT']);
      cancelled.abort();await Promise.all(jobs);
      const read=b.readPiece(controller.signal),sending=a.sendPiece(0n,0,new Uint8Array([9]),controller.signal);
      blocked=false;channel.dispatchEvent(new Event('bufferedamountlow'));await sending;assert((await read).data[0]===9);
      blocked=true;const closing=rejects(a.sendPiece(0n,0,new Uint8Array(1),controller.signal),['PEER_CLOSED']);await a.close();await closing;return;
    }
    if(options.overflow){
      for(let i=0;i<300;i++){try{await a.sendControl('{"v":1,"message":{"type":"request_info"}}',controller.signal);}catch{break;}}
      const until=performance.now()+5000;
      while(!(b as unknown as {stopped?:Q2QError}).stopped&&performance.now()<until)await new Promise(resolve=>setTimeout(resolve,10));
      await rejects(b.readControl(controller.signal),['RESOURCE_LIMIT']);return;
    }
    const raw='{"v":1,"message":{"type":"have","block_index":18446744073709551615}}';
    await a.sendControl(raw,controller.signal);assert(await b.readControl(controller.signal)===raw);
    await b.sendControl('{"v":1,"message":{"type":"request_info"}}',controller.signal);assert((await a.readControl(controller.signal)).includes('request_info'));
    const payload=new Uint8Array(1024);for(let i=0;i<payload.length;i++)payload[i]=i%251;
    const wait=b.readPiece(controller.signal);await a.sendPiece((1n<<64n)-1n,1024,payload,controller.signal);
    const piece=await wait;assert(piece.blockIndex===(1n<<64n)-1n&&piece.offset===1024&&piece.data.every((v,i)=>v===payload[i]));
    await b.sendPiece(1n,0,payload.subarray(0,31),controller.signal);assert((await a.readPiece(controller.signal)).data.length===31);
    await rejects(a.sendPiece(-1n,0,payload,controller.signal),['PEER_PROTOCOL_ERROR']);
    await rejects(a.sendPiece(1n,1,payload,controller.signal),['PEER_PROTOCOL_ERROR']);
    await rejects(a.sendControl('{"v":2,"message":{"type":"request_info"}}',controller.signal),['PEER_PROTOCOL_ERROR']);
    const cancelled=new AbortController();const waiting=rejects(a.readControl(cancelled.signal),['TASK_CANCELLED']);cancelled.abort();await waiting;
    await a.close();await rejects(a.readPiece(controller.signal),['PEER_CLOSED']);
  }finally{controller.abort();await Promise.all([a?.close(),b?.close()]);one.close();two.close();}
}
export async function timeout(){
  const identity=await controlIdentity();const abort=new AbortController();
  const peer=await RtcPeer.create({identity,remoteKey:decodeKey(identity.info.public_key),infoHash:new Uint8Array(32),connectionId:'timeout',initiator:true,signal:abort.signal,onCandidate:()=>{},timeoutMs:50});
  try{await peer.offer();await rejects(peer.ready(),['CONTROL_TIMEOUT']);}finally{await peer.close();identity.close();}
}

export async function nativeExchange(nativePublicKey:string,nativeInitiator:boolean){
  const bridge=globalThis as unknown as {nativeRead():Promise<Record<string,unknown>>;nativeSend(value:unknown):Promise<void>};
  const identity=await controlIdentity();const controller=new AbortController();let descriptionsSent=false;const candidates:RTCIceCandidateInit[]=[];
  const errors:unknown[]=[];
  const peer=await RtcPeer.create({identity,remoteKey:decodeKey(nativePublicKey),infoHash:new Uint8Array(32).fill(3),connectionId:'native-browser',initiator:!nativeInitiator,signal:controller.signal,
    onCandidate:c=>{if(descriptionsSent)void bridge.nativeSend({type:'candidate',candidate:c}).catch(error=>errors.push(error));else candidates.push(c);}});
  let nativeAuthenticated=false;let echoed=false;
  async function description(type:string,description:RTCSessionDescriptionInit){
    await bridge.nativeSend({type,description});descriptionsSent=true;
    for(const candidate of candidates.splice(0))await bridge.nativeSend({type:'candidate',candidate});
  }
  await bridge.nativeSend({type:'init',public_key:identity.info.public_key,initiator:nativeInitiator});
  const loop=(async()=>{
    while(!echoed){
      const value=await bridge.nativeRead();
      if(value.type==='offer')await description('answer',await peer.answer(value.description as RTCSessionDescriptionInit));
      else if(value.type==='answer')await peer.applyAnswer(value.description as RTCSessionDescriptionInit);
      else if(value.type==='candidate')await peer.addCandidate(JSON.parse(value.candidate as string));
      else if(value.type==='authenticated')nativeAuthenticated=true;
      else if(value.type==='echoed')echoed=true;
    }
  })();void loop.catch(async error=>{errors.push(error);const pc=(peer as unknown as {pc:RTCPeerConnection}).pc;errors.push(JSON.stringify({ice:pc.iceConnectionState,state:pc.connectionState,sctp:pc.sctp?.state,stats:Array.from((await pc.getStats()).values()).filter(row=>['transport','candidate-pair','local-candidate','remote-candidate','data-channel'].includes(row.type))}));controller.abort();});
  try{
    if(!nativeInitiator)await description('offer',await peer.offer());
    await peer.ready();
    const raw='{"v":1,"message":{"type":"have","block_index":18446744073709551615}}';
    const control=peer.readControl(controller.signal),piece=peer.readPiece(controller.signal);
    await peer.sendControl(raw,controller.signal);
    const payload=new Uint8Array(1024);for(let i=0;i<payload.length;i++)payload[i]=i%251;
    await peer.sendPiece((1n<<64n)-1n,1024,payload,controller.signal);
    const [response,bytes]=await Promise.all([control,piece]);assert(response===raw);
    assert(bytes.blockIndex===(1n<<64n)-1n&&bytes.offset===1024&&bytes.data.every((v,i)=>v===payload[i]));
    await loop;assert(nativeAuthenticated&&echoed&&errors.length===0);
    await bridge.nativeSend({type:'close'});
  }catch(error){if(errors.length){const pc=(peer as unknown as {pc:RTCPeerConnection}).pc;throw Error(errors.map(String).join('\n')+' states '+JSON.stringify({ice:pc.iceConnectionState,state:pc.connectionState,sctp:pc.sctp?.state,stats:Array.from((await pc.getStats()).values()).filter(row=>['transport','candidate-pair','local-candidate','remote-candidate','data-channel'].includes(row.type))}));}throw error;}finally{controller.abort();await peer.close();identity.close();}
}
