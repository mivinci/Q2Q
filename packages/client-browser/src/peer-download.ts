import { Q2QError } from './error.ts';
import { PermitPool } from './http.ts';
import { hasBlock,setBlock } from './checkpoint-codec.ts';
import { initializeWasm } from './block-verifier.ts';
import type { PeerConnection,PeerCandidate } from './peer-dial.ts';
import type { PeerNetwork } from './network.ts';
import type { ContentInfo } from './client-types.ts';
type Wasm=Awaited<ReturnType<typeof initializeWasm>>;
type Message={type:string;block_index?:string;count?:number;retry_after?:number};
type Active={index:number;assembly:InstanceType<Wasm['PeerBlockAssembly']>;proof:boolean;delivered:boolean;
  proofFrame?:string;witness?:Uint8Array;verify(bytes:Uint8Array,proof:string):boolean|Uint8Array;resolve(bytes:Uint8Array):void;reject(error:Q2QError):void;retry?:ReturnType<typeof setTimeout>;deadline?:ReturnType<typeof setTimeout>;attempt:number};
export interface PeerBlock {bytes:Uint8Array;witness?:Uint8Array;acknowledge():Promise<void>}
export interface PeerSource {fetch(index:number,length:number,verify:(bytes:Uint8Array,proof:string)=>boolean|Uint8Array):Promise<PeerBlock>;close():Promise<void>}
const error=(value:unknown)=>value instanceof Q2QError?value:new Q2QError('PEER_PROTOCOL_ERROR');
const encode=(message:unknown)=>JSON.stringify({v:1,message});

/** One native-compatible single-block window. Readers keep draining while disk
 * commit runs, so retransmitted pieces cannot accumulate behind storage IO. */
class DownloadConnection {
  private connection:PeerConnection;private wasm:Wasm;private bits:Uint8Array;private available:Uint8Array;private count:number;
  private controller=new AbortController();private parent:AbortSignal;private abort:()=>void;
  private active?:Active;private failure?:Q2QError;private loops:Promise<unknown>[]=[];private retryWork?:Promise<void>;
  private closing?:Promise<void>;private finishAck?:()=>void;
  private received:(bytes:number)=>void;
  private constructor(connection:PeerConnection,wasm:Wasm,bits:Uint8Array,available:Uint8Array,count:number,signal:AbortSignal,received:(bytes:number)=>void){
    this.connection=connection;this.wasm=wasm;this.bits=bits;this.available=available;this.count=count;this.received=received;this.parent=signal;
    this.abort=()=>this.fail(new Q2QError('TASK_CANCELLED'));signal.addEventListener('abort',this.abort,{once:true});
    this.loops=[this.controls(),this.pieces()];for(const work of this.loops)void work.catch(value=>this.fail(error(value)));
    if(signal.aborted)this.abort();
  }
  static async open(connection:PeerConnection,wasm:Wasm,bits:Uint8Array,count:number,signal:AbortSignal,received:(bytes:number)=>void){
    const timeout=AbortSignal.timeout(10000),combined=AbortSignal.any([signal,timeout]);
    try{
      await connection.rtc.sendControl(wasm.encode_peer_bitfield(bits,count),combined);
      const raw=await connection.rtc.readControl(combined);const available=wasm.peer_bitfield(raw,count);
      return new DownloadConnection(connection,wasm,bits,available,count,signal,received);
    }catch(value){
      const failure=connection.rtc.diagnosed(timeout.aborted&&!signal.aborted?new Q2QError('CONTROL_TIMEOUT',true):error(value));
      await connection.close();throw failure;
    }
  }
  has(index:number){return hasBlock(this.available,index);}
  private send(message:unknown){return this.connection.rtc.sendControl(encode(message),this.controller.signal);}
  private fail(problem:Q2QError){
    if(this.failure)return;problem=this.connection.rtc.diagnosed(problem);this.failure=problem;this.controller.abort();
    if(this.active){clearTimeout(this.active.retry);clearTimeout(this.active.deadline);this.active.reject(problem);}
    void this.connection.close();
  }
  private progress(active:Active,delay=20000){clearTimeout(active.deadline);active.deadline=setTimeout(()=>this.fail(new Q2QError('CONTROL_TIMEOUT',true)),delay);}
  private retry(active:Active,delay=500){
    clearTimeout(active.retry);active.retry=setTimeout(()=>{
      if(this.failure||this.active!==active||active.delivered)return;
      const work=(async()=>{
        if(!active.proof)await this.send({type:'request_proof',block_index:active.index,count:1});
        const ranges=JSON.parse(active.assembly.missing()) as [number,number][];
        for(let i=0;i<ranges.length;i+=64){const batch=ranges.slice(i,i+64);await this.send({type:'piece_nack',block_index:active.index,piece_offset:batch.map(v=>v[0]),length:batch.map(v=>v[1])});}
      })();this.retryWork=work;
      void work.catch(value=>this.fail(error(value))).finally(()=>{
        if(this.retryWork===work)this.retryWork=undefined;
        if(!this.failure&&this.active===active&&!active.delivered)this.retry(active,500*2**Math.min(++active.attempt,3));
      });
    },delay);
  }
  private complete(active:Active){
    if(active.delivered||!active.proof||!active.assembly.complete())return;
    const bytes=active.assembly.bytes();const verified=active.verify(bytes,active.proofFrame!);if(!verified){this.fail(new Q2QError('CONTENT_MISMATCH'));return;}
    if(verified instanceof Uint8Array)active.witness=verified;
    active.delivered=true;clearTimeout(active.retry);clearTimeout(active.deadline);active.resolve(bytes);
  }
  private async controls(){
    for(;;){
      const raw=await this.connection.rtc.readControl(this.controller.signal);const value=JSON.parse(this.wasm.decode_peer_control(raw)) as Message;
      const index=value.block_index===undefined?undefined:BigInt(value.block_index);const active=this.active;
      if(value.type==='have'&&index!==undefined&&index<BigInt(this.count)) {setBlock(this.available,Number(index));continue;}
      if(value.type==='cancel'&&index===0n&&value.count===0&&this.finishAck){this.finishAck();continue;}
      if(index!==undefined&&index>=0n&&index<BigInt(this.count)&&hasBlock(this.bits,Number(index)))continue;
      if(!active||index!==BigInt(active.index))throw new Q2QError('PEER_PROTOCOL_ERROR');
      if(value.type==='proof'&&value.count===1){active.proof=true;active.proofFrame=raw;this.complete(active);}
      else if(value.type==='upload_wait'&&typeof value.retry_after==='number'){
        if(!active.delivered){this.progress(active,Math.max(20000,value.retry_after));this.retry(active,Math.max(500,value.retry_after));}
      }else if(value.type==='reject')throw new Q2QError('SOURCE_UNAVAILABLE');
      else throw new Q2QError('PEER_PROTOCOL_ERROR');
    }
  }
  private async pieces(){
    for(;;){
      const piece=await this.connection.rtc.readPiece(this.controller.signal);
      if(piece.blockIndex<BigInt(this.count)&&hasBlock(this.bits,Number(piece.blockIndex)))continue;
      const active=this.active;if(!active||piece.blockIndex!==BigInt(active.index))throw new Q2QError('PEER_PROTOCOL_ERROR');
      const fresh=active.assembly.insert(piece.offset,piece.data);this.received(piece.data.length);
      if(fresh&&!active.delivered){active.attempt=0;this.progress(active);this.retry(active);this.complete(active);}
    }
  }
  async fetch(index:number,length:number,verify:(bytes:Uint8Array,proof:string)=>boolean|Uint8Array):Promise<PeerBlock>{
    if(this.failure)throw this.failure;if(this.active)throw new Q2QError('INVALID_STATE');
    let active!:Active;
    const result=new Promise<Uint8Array>((resolve,reject)=>{active={index,assembly:new this.wasm.PeerBlockAssembly(length),proof:false,delivered:false,verify,resolve,reject,attempt:0};});
    void result.catch(()=>{});this.active=active;this.progress(active);this.retry(active);
    try{await this.send({type:'request',block_index:index,count:1});const bytes=await result;
      return {bytes,witness:active.witness,acknowledge:async()=>{
        try{if(!this.failure){await this.send({type:'have',block_index:index});await this.send({type:'block_done',block_index:index});}setBlock(this.bits,index);}
        catch(value){this.fail(error(value));}
        finally{if(this.active===active){this.active=undefined;await this.retryWork?.catch(()=>{});active.assembly.free();}}
      }};
    }catch(value){this.fail(error(value));throw this.failure;}
  }
  close(graceful=false):Promise<void>{
    if(this.closing)return this.closing;
    this.closing=(async()=>{
      try{
        if(graceful&&!this.failure&&!this.active){
          const ack=new Promise<void>(resolve=>{this.finishAck=resolve;});
          const timer=setTimeout(()=>this.finishAck?.(),1000);
          try{await this.send({type:'cancel',block_index:0,count:0});await ack;}finally{clearTimeout(timer);}
        }
      }finally{
        this.fail(new Q2QError('PEER_CLOSED'));this.parent.removeEventListener('abort',this.abort);
        await Promise.allSettled([...this.loops,...(this.retryWork?[this.retryWork]:[]),this.connection.close()]);
        this.active?.assembly.free();this.active=undefined;
      }
    })();return this.closing;
  }
}

export async function peerSource(options:{network:PeerNetwork;content:ContentInfo;committed:Uint8Array;signal:AbortSignal;permits:PermitPool;received(bytes:number):void}):Promise<PeerSource>{
  if(options.content.blockCount>65536n)throw new Q2QError('UNSUPPORTED_CAPABILITY');
  const wasm=await initializeWasm(),hash=new Uint8Array(options.content.id.slice(7).match(/../g)!.map(v=>parseInt(v,16)));
  const committed=options.committed.slice();
  let candidates=await options.network.query(options.content.id,options.signal);
  // Track's cross-object file index converges after publication announce.
  for(let retry=0;candidates.length===0&&retry<3;retry++){
    await new Promise<void>((resolve,reject)=>{
      const stop=()=>{clearTimeout(timer);reject(new Q2QError('TASK_CANCELLED'));};
      const timer=setTimeout(()=>{options.signal.removeEventListener('abort',stop);resolve();},250);
      options.signal.addEventListener('abort',stop,{once:true});if(options.signal.aborted)stop();
    });
    candidates=await options.network.query(options.content.id,options.signal);
  }let cursor=0;let current:DownloadConnection|undefined;let releasePeer:(()=>void)|undefined;
  const serial=new PermitPool(1);let held:(()=>void)|undefined;let closed=false;let exhausted:Q2QError|undefined;
  async function retire(graceful=false){try{await current?.close(graceful);}finally{current=undefined;releasePeer?.();releasePeer=undefined;}}
  return {
    async fetch(index,length,verify){
      const release=await serial.acquire(options.signal);held=release;
      try{
        let last=exhausted??new Q2QError('NO_SOURCES');
        for(;;){
          if(closed||options.signal.aborted)throw new Q2QError('TASK_CANCELLED');
          try{
            if(!current){
              const candidate:PeerCandidate|undefined=candidates[cursor++];if(!candidate)throw last;
              releasePeer=await options.permits.acquire(options.signal);
              const connection=await options.network.dial(candidate,hash,options.signal);
              current=await DownloadConnection.open(connection,wasm,committed,Number(options.content.blockCount),options.signal,options.received);
            }
            if(!current.has(index))throw new Q2QError('SOURCE_UNAVAILABLE');
            const block=await current.fetch(index,length,verify);
            return {bytes:block.bytes,witness:block.witness,async acknowledge(){try{await block.acknowledge();}finally{held=undefined;release();}}};
          }catch(value){last=error(value);await retire();if(cursor>=Math.min(candidates.length,8)){exhausted=last;throw last;}}
        }
      }catch(value){held=undefined;release();throw value;}
    },
    async close(){closed=true;try{await retire(!options.signal.aborted);}finally{held?.();held=undefined;}}
  };
}
