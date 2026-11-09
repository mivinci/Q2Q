import { Q2QError, type ConnectionDiagnostic } from './error.ts';
import { initializeWasm } from './block-verifier.ts';
import type { ControlIdentity } from './control-identity.ts';

const CONTROL_MAX=16384, PIECE_MAX=1041, BUDGET=256*1024;
type Wasm=Awaited<ReturnType<typeof initializeWasm>>;
type Auth=ReturnType<ControlIdentity['beginPeer']>;
export type PeerPiece=Readonly<{blockIndex:bigint;offset:number;data:Uint8Array}>;
export interface RtcPeerOptions {
  identity:ControlIdentity; remoteKey:Uint8Array; infoHash:Uint8Array; connectionId:string; initiator:boolean;
  signal:AbortSignal; iceServers?:RTCIceServer[]; onCandidate(candidate:RTCIceCandidateInit):void|Promise<void>;
  timeoutMs?:number;
}

/** A bounded single-consumer inbox. Incoming events never wait for application IO. */
export class Inbox<T> {
  private queue:{value:T;size:number}[]=[];private bytes=0;private error?:Q2QError;
  private pending?:{resolve(value:T):void;reject(error:Q2QError):void;clean():void};
  push(value:T,size:number){
    if(this.error)return;
    if(this.pending){const pending=this.pending;this.pending=undefined;pending.clean();pending.resolve(value);return;}
    if(this.queue.length>=256||this.bytes+size>BUDGET)throw new Q2QError('RESOURCE_LIMIT');
    this.queue.push({value,size});this.bytes+=size;
  }
  read(signal:AbortSignal):Promise<T>{
    if(signal.aborted)return Promise.reject(new Q2QError('TASK_CANCELLED'));
    if(this.error)return Promise.reject(this.error);
    if(this.pending)return Promise.reject(new Q2QError('INVALID_STATE'));
    const item=this.queue.shift();if(item){this.bytes-=item.size;return Promise.resolve(item.value);}
    return new Promise((resolve,reject)=>{
      const clean=()=>signal.removeEventListener('abort',abort);
      const abort=()=>{this.pending=undefined;clean();reject(new Q2QError('TASK_CANCELLED'));};
      this.pending={resolve,reject,clean};signal.addEventListener('abort',abort,{once:true});
    });
  }
  close(error:Q2QError){this.error=error;this.queue=[];this.bytes=0;this.pending?.clean();this.pending?.reject(error);this.pending=undefined;}
}

/** Browser host transport, internal until publication/download scheduling is wired.
 * Control is reliable/ordered; pieces use native negotiated stream 1, unordered,
 * maxRetransmits=0. Q2Q's block/proof/retransmission logic remains above this layer.
 */
export class RtcPeer {
  private stage:ConnectionDiagnostic['stage']='rtc-connect';
  private localCandidateCount=0;private remoteCandidateCount=0;
  private pc:RTCPeerConnection;private control?:RTCDataChannel;private data:RTCDataChannel;
  private wasm:Wasm;private options:RtcPeerOptions;private auth?:Auth;private early?:string;
  private authenticated=false;private stopped?:Q2QError;private localAbort=new AbortController();
  private readyWork:Promise<void>;private resolveReady!:()=>void;private rejectReady!:(error:Q2QError)=>void;
  private timer:ReturnType<typeof setTimeout>;private abort:()=>void;
  private controls=new Inbox<string>();private pieces=new Inbox<PeerPiece>();
  private sends=new Set<Promise<void>>();private queuedBytes=0;private candidates:RTCIceCandidateInit[]=[];
  private notifications=new Set<Promise<unknown>>();
  private candidateChain=Promise.resolve();private settingRemote=false;
  private constructor(options:RtcPeerOptions,wasm:Wasm){
    this.options={...options,remoteKey:options.remoteKey.slice(),infoHash:options.infoHash.slice()};this.wasm=wasm;
    this.pc=new RTCPeerConnection({iceServers:options.iceServers??[]});
    this.readyWork=new Promise((resolve,reject)=>{this.resolveReady=resolve;this.rejectReady=reject;});
    void this.readyWork.catch(()=>{});
    this.timer=setTimeout(()=>this.stop(new Q2QError('CONTROL_TIMEOUT',true)),options.timeoutMs??20000);
    this.abort=()=>this.stop(new Q2QError('TASK_CANCELLED'));options.signal.addEventListener('abort',this.abort,{once:true});
    this.data=this.pc.createDataChannel('q2q-data',{negotiated:true,id:1,ordered:false,maxRetransmits:0});this.bind(this.data,false);
    this.pc.addEventListener('datachannel',event=>{
      if(this.control||event.channel.label!=='q2q-control'){event.channel.close();this.stop(new Q2QError('PEER_PROTOCOL_ERROR'));return;}
      this.control=event.channel;this.bind(event.channel,true);
    });
    this.pc.addEventListener('icecandidate',event=>{
      if(!event.candidate||this.stopped)return;
      this.localCandidateCount++;
      if(this.notifications.size>=64){this.stop(new Q2QError('RESOURCE_LIMIT'));return;}
      const work=Promise.resolve().then(()=>{if(!this.stopped)return options.onCandidate(event.candidate!.toJSON());});
      this.notifications.add(work);void work.catch(()=>this.stop(new Q2QError('NETWORK_UNAVAILABLE',true))).finally(()=>this.notifications.delete(work));
    });
    this.pc.addEventListener('connectionstatechange',()=>{
      if(['failed','closed'].includes(this.pc.connectionState))this.stop(new Q2QError('PEER_CLOSED',true));
    });
    if(options.signal.aborted)this.abort();
  }
  static async create(options:RtcPeerOptions):Promise<RtcPeer>{
    if(options.signal.aborted)throw new Q2QError('TASK_CANCELLED');
    if(options.remoteKey.length!==32||options.infoHash.length!==32||!/^[-\w.]{1,128}$/.test(options.connectionId))throw new Q2QError('INVALID_ARGUMENT');
    const wasm=await initializeWasm();if(options.signal.aborted)throw new Q2QError('TASK_CANCELLED');
    return new RtcPeer(options,wasm);
  }
  private bind(channel:RTCDataChannel,control:boolean){
    channel.binaryType='arraybuffer';channel.bufferedAmountLowThreshold=64*1024;
    channel.addEventListener('open',()=>this.startAuth());
    channel.addEventListener('close',()=>this.stop(new Q2QError('PEER_CLOSED')));
    channel.addEventListener('error',()=>this.stop(new Q2QError('PEER_CLOSED',true)));
    channel.addEventListener('message',event=>{
      if(this.stopped)return;
      try{
        if(control){
          if(typeof event.data!=='string'||new TextEncoder().encode(event.data).length>CONTROL_MAX)throw new Q2QError('PEER_PROTOCOL_ERROR');
          if(!this.authenticated){
            if(!this.auth){
              if(this.early!==undefined||event.data.length>512)throw new Q2QError('PEER_AUTH_FAILED');
              this.early=event.data;return;
            }
            this.authenticate(event.data);return;
          }
          this.wasm.validate_peer_control(event.data);this.controls.push(event.data,new TextEncoder().encode(event.data).length);
        }else{
          if(!this.authenticated)throw new Q2QError('PEER_AUTH_FAILED');
          if(!(event.data instanceof ArrayBuffer)||event.data.byteLength>PIECE_MAX)throw new Q2QError('PEER_PROTOCOL_ERROR');
          const bytes=new Uint8Array(event.data);const header=JSON.parse(this.wasm.peer_piece_header(bytes)) as {block_index:string;piece_offset:number};
          this.pieces.push(Object.freeze({blockIndex:BigInt(header.block_index),offset:header.piece_offset,data:bytes.subarray(17)}),bytes.length);
        }
      }catch(error){this.stop(error instanceof Q2QError?error:new Q2QError(this.authenticated?'PEER_PROTOCOL_ERROR':'PEER_AUTH_FAILED'));}
    });
  }
  private startAuth(){
    if(this.stopped||this.authenticated||this.auth||this.control?.readyState!=='open'||this.data.readyState!=='open'||!this.pc.localDescription||!this.pc.remoteDescription)return;
    this.stage='peer-auth';
    try{
      if(!this.control.ordered||this.control.maxRetransmits!==null||this.control.maxPacketLifeTime!==null)throw new Q2QError('PEER_PROTOCOL_ERROR');
      const max=this.pc.sctp?.maxMessageSize;
      if(max===undefined||max!==0&&max<CONTROL_MAX)throw new Q2QError('UNSUPPORTED_CAPABILITY');
      this.auth=this.options.identity.beginPeer(this.options.remoteKey,this.options.initiator,this.options.infoHash,this.options.connectionId,this.pc.localDescription.sdp,this.pc.remoteDescription.sdp);
      this.control.send(this.auth.hello());
      if(this.early!==undefined){const early=this.early;this.early=undefined;this.authenticate(early);}
    }catch(error){this.stop(error instanceof Q2QError?error:new Q2QError('PEER_AUTH_FAILED'));}
  }
  private authenticate(raw:string){
    try{
      const reply=this.auth!.receive(raw);if(reply)this.control!.send(reply);
      if(this.auth!.authenticated()){
        this.authenticated=true;this.stage='transfer';this.auth!.free();this.auth=undefined;clearTimeout(this.timer);this.resolveReady();
      }
    }catch{throw new Q2QError('PEER_AUTH_FAILED');}
  }
  private check(){if(this.stopped)throw this.stopped;}
  async offer():Promise<RTCSessionDescriptionInit>{
    this.check();if(!this.options.initiator||this.control)throw new Q2QError('INVALID_STATE');
    try{this.control=this.pc.createDataChannel('q2q-control',{ordered:true});this.bind(this.control,true);
      await this.pc.setLocalDescription(await this.pc.createOffer());this.check();return this.pc.localDescription!.toJSON();
    }catch(error){this.stop(this.diagnosed(error,'rtc-offer'));throw this.stopped;}
  }
  async answer(offer:RTCSessionDescriptionInit):Promise<RTCSessionDescriptionInit>{
    this.check();if(this.options.initiator||this.pc.remoteDescription)throw new Q2QError('INVALID_STATE');
    try{await this.remote(offer,'offer');await this.pc.setLocalDescription(await this.pc.createAnswer());this.check();this.startAuth();return this.pc.localDescription!.toJSON();}
    catch(error){this.stop(this.diagnosed(error,'rtc-answer'));throw this.stopped;}
  }
  async applyAnswer(answer:RTCSessionDescriptionInit):Promise<void>{
    this.check();if(!this.options.initiator||!this.pc.localDescription||this.pc.remoteDescription)throw new Q2QError('INVALID_STATE');
    try{await this.remote(answer,'answer');this.startAuth();}catch(error){this.stop(this.diagnosed(error,'rtc-answer'));throw this.stopped;}
  }
  private async remote(description:RTCSessionDescriptionInit,type:'offer'|'answer'){
    if(description.type!==type||typeof description.sdp!=='string'||new TextEncoder().encode(description.sdp).length>65536)throw new Q2QError('PEER_PROTOCOL_ERROR');
    this.settingRemote=true;
    try{await this.pc.setRemoteDescription(description);for(const candidate of this.candidates)await this.applyCandidate(candidate);this.candidates=[];}
    catch(error){throw this.diagnosed(error,'rtc-remote-description');}
    finally{this.settingRemote=false;}
  }
  private async applyCandidate(candidate:RTCIceCandidateInit):Promise<void>{
    if((candidate.sdpMid===null||candidate.sdpMid==='')&&candidate.sdpMLineIndex!=null){
      const section=this.pc.remoteDescription?.sdp.split(/^m=/m)[candidate.sdpMLineIndex+1];
      const mid=section?.match(/^a=mid:([^\r\n]+)/m)?.[1];
      if(mid)candidate={...candidate,sdpMid:mid};
    }
    try{await this.pc.addIceCandidate(candidate);}
    catch(error){throw this.diagnosed(error,'rtc-candidate');}
  }
  addCandidate(candidate:RTCIceCandidateInit):Promise<void>{
    this.check();this.remoteCandidateCount++;
    // Native webrtc-rs reports an empty MID with a valid m-line index. Treat it
    // as absent and use the m-line index for browser interoperability.
    if(candidate.sdpMid===''&&candidate.sdpMLineIndex!=null)candidate={...candidate,sdpMid:null};
    if(typeof candidate.candidate!=='string'||new TextEncoder().encode(candidate.candidate).length>4096||this.candidates.length>=64)return Promise.reject(new Q2QError('RESOURCE_LIMIT'));
    if(!this.pc.remoteDescription||this.settingRemote){this.candidates.push({...candidate});return Promise.resolve();}
    // Serialize host candidate IO without keeping an unbounded promise chain.
    if(this.candidateCount>=64)return Promise.reject(new Q2QError('RESOURCE_LIMIT'));
    this.candidateCount++;
    const work=this.candidateChain.then(()=>{this.check();return this.applyCandidate(candidate);});
    const checked=work.catch(error=>{this.stop(error instanceof Q2QError?error:new Q2QError('PEER_PROTOCOL_ERROR'));throw this.stopped;});
    this.candidateChain=checked.catch(()=>{});return checked.finally(()=>{this.candidateCount--;});
  }
  private candidateCount=0;
  ready():Promise<void>{return this.readyWork;}
  readControl(signal:AbortSignal):Promise<string>{return this.controls.read(signal);}
  readPiece(signal:AbortSignal):Promise<PeerPiece>{return this.pieces.read(signal);}
  sendControl(raw:string,signal:AbortSignal):Promise<void>{
    try{this.wasm.validate_peer_control(raw);}catch{return Promise.reject(new Q2QError('PEER_PROTOCOL_ERROR'));}
    return this.send(this.control,raw,new TextEncoder().encode(raw).length,signal);
  }
  sendPiece(block:bigint,offset:number,data:Uint8Array,signal:AbortSignal):Promise<void>{
    if(block<0n||block>=(1n<<64n)||!Number.isSafeInteger(offset)||offset<0||offset>0xffffffff)return Promise.reject(new Q2QError('PEER_PROTOCOL_ERROR'));
    let encoded:Uint8Array;
    try{encoded=this.wasm.encode_peer_piece(block,offset,data);}catch{return Promise.reject(new Q2QError('PEER_PROTOCOL_ERROR'));}
    return this.send(this.data,encoded,encoded.length,signal);
  }
  private send(channel:RTCDataChannel|undefined,payload:string|Uint8Array,size:number,signal:AbortSignal):Promise<void>{
    if(signal.aborted)return Promise.reject(new Q2QError('TASK_CANCELLED'));
    if(this.stopped)return Promise.reject(this.stopped);
    if(!this.authenticated||!channel)return Promise.reject(new Q2QError('INVALID_STATE'));
    if(this.sends.size>=8||this.queuedBytes+size>BUDGET)return Promise.reject(new Q2QError('RESOURCE_LIMIT'));
    this.queuedBytes+=size;
    const work=(async()=>{
      while(channel.bufferedAmount+size>BUDGET){
        await new Promise<void>((resolve,reject)=>{
          const clean=()=>{clearTimeout(timer);channel.removeEventListener('bufferedamountlow',low);signal.removeEventListener('abort',abort);this.localAbort.signal.removeEventListener('abort',closed);};
          const low=()=>{clean();resolve();};const abort=()=>{clean();reject(new Q2QError('TASK_CANCELLED'));};const closed=()=>{clean();reject(this.stopped!);};
          const timer=setTimeout(()=>{clean();reject(new Q2QError('CONTROL_TIMEOUT',true));},10000);
          channel.addEventListener('bufferedamountlow',low,{once:true});signal.addEventListener('abort',abort,{once:true});this.localAbort.signal.addEventListener('abort',closed,{once:true});
          if(signal.aborted)abort();else if(this.stopped)closed();else if(channel.bufferedAmount+size<=BUDGET)low();
        });
      }
      this.check();if(signal.aborted)throw new Q2QError('TASK_CANCELLED');
      try{if(typeof payload==='string')channel.send(payload);else channel.send(payload as Uint8Array<ArrayBuffer>);}
      catch{throw new Q2QError('PEER_CLOSED',true);}
    })();
    this.sends.add(work);void work.finally(()=>{this.sends.delete(work);this.queuedBytes-=size;}).catch(()=>{});return work;
  }
  /** Internal: capture host state before an owner starts connection cleanup. */
  diagnosed(value:unknown,stage:ConnectionDiagnostic['stage']=this.stage):Q2QError {
    const error=value instanceof Q2QError?value:new Q2QError('PEER_PROTOCOL_ERROR');
    if(error.diagnostic)return error;
    return new Q2QError(error.code,error.retryable,{
      stage,connectionState:this.pc.connectionState,iceConnectionState:this.pc.iceConnectionState,
      iceGatheringState:this.pc.iceGatheringState,signalingState:this.pc.signalingState,
      controlState:this.control?.readyState,dataState:this.data.readyState,
      localCandidates:this.localCandidateCount,remoteCandidates:this.remoteCandidateCount
    });
  }
  private stop(error:Q2QError){
    if(this.stopped)return;error=this.diagnosed(error);this.stopped=error;clearTimeout(this.timer);this.options.signal.removeEventListener('abort',this.abort);
    this.localAbort.abort();this.rejectReady(error);this.controls.close(error);this.pieces.close(error);this.candidates=[];this.early=undefined;
    this.auth?.free();this.auth=undefined;this.control?.close();this.data.close();this.pc.close();
  }
  async close():Promise<void>{this.stop(new Q2QError('PEER_CLOSED'));await Promise.allSettled([...this.sends,...this.notifications,this.candidateChain]);}
}
