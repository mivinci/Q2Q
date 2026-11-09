import { Q2QError } from './error.ts';
import { cancelled } from './http.ts';
import { endpoint } from './control-identity.ts';
import type { ControlIdentity } from './control-identity.ts';

export type SignalMessage = Readonly<{type:'connect'|'accept'|'reject'|'offer'|'answer'|'candidate'|'cancel';connection_id:string;
  connect_token?:string|null;accept_token?:string|null;sdp?:string|null;candidate?:string|null;reason?:string|null;signal_key?:string|null}>;
type Pending={resolve(value:unknown):void;reject(error:Q2QError):void;clean():void};
const LIMIT=64*1024;
/** Browser host adapter only. Rust validates the existing binary frame protocol. */
export class PunchSocket {
  private socket:WebSocket;
  private pending=new Map<string,Pending>();
  private signals=new Set<Promise<unknown>>();
  private ended=false;
  private closing?:Promise<void>;
  private identity:ControlIdentity;
  private onSignal:(value:SignalMessage)=>void|Promise<void>;
  private onFailure:(error:Q2QError)=>void;
  private constructor(url:string,identity:ControlIdentity,onSignal:(value:SignalMessage)=>void|Promise<void>,onFailure:(error:Q2QError)=>void) {
    this.identity=identity;this.onSignal=onSignal;this.onFailure=onFailure;
    this.socket=new WebSocket(endpoint(url,'websocket'));
    this.socket.binaryType='arraybuffer';
    this.socket.addEventListener('message',event=>this.receive(event.data));
    this.socket.addEventListener('close',()=>this.fail(new Q2QError('NETWORK_UNAVAILABLE',true)));
    this.socket.addEventListener('error',()=>this.fail(new Q2QError('NETWORK_UNAVAILABLE',true)));
  }
  static async open(options:{url:string;identity:ControlIdentity;signal:AbortSignal;timeoutMs?:number;onSignal:(value:SignalMessage)=>void|Promise<void>;onFailure:(error:Q2QError)=>void}) {
    if(options.signal.aborted)throw cancelled();
    const channel=new PunchSocket(options.url,options.identity,options.onSignal,options.onFailure);
    try {
      await new Promise<void>((resolve,reject)=>{
        const timer=setTimeout(()=>stop(new Q2QError('CONTROL_TIMEOUT',true)),options.timeoutMs??10000);
        const clean=()=>{clearTimeout(timer);options.signal.removeEventListener('abort',abort);channel.socket.removeEventListener('open',open);channel.socket.removeEventListener('close',closed);channel.socket.removeEventListener('error',closed);};
        const stop=(error:Q2QError)=>{clean();reject(error);};
        const abort=()=>stop(cancelled());
        const open=()=>{clean();resolve();};
        const closed=()=>stop(new Q2QError('NETWORK_UNAVAILABLE',true));
        options.signal.addEventListener('abort',abort,{once:true});channel.socket.addEventListener('open',open,{once:true});channel.socket.addEventListener('close',closed,{once:true});channel.socket.addEventListener('error',closed,{once:true});
        if(options.signal.aborted)abort();
      });
      return channel;
    } catch(error) {await channel.close();throw error;}
  }
  private fail(error:Q2QError) {
    if(this.ended)return;this.ended=true;
    for(const pending of this.pending.values()){pending.clean();pending.reject(error);}this.pending.clear();
    try{this.socket.close();}catch{/* A connecting socket may already be closing. */}
    try{this.onFailure(error);}catch{/* Observers do not own the transport. */}
  }
  private receive(data:unknown) {
    if(this.ended)return;
    try {
      if(!(data instanceof ArrayBuffer)||data.byteLength>LIMIT)throw new Q2QError('NETWORK_PROTOCOL_ERROR');
      const frame=JSON.parse(this.identity.decodeFrame(new Uint8Array(data))) as {type:string;payload:Record<string,unknown>};
      if(frame.type==='response') {
        const id=frame.payload.id;if(typeof id!=='string')throw new Q2QError('NETWORK_PROTOCOL_ERROR');
        const pending=this.pending.get(id);
        if(!pending)return; // Replies to timed-out/aborted calls cannot revive them.
        this.pending.delete(id);pending.clean();
        if(Boolean(frame.payload.error) === (frame.payload.result!==undefined&&frame.payload.result!==null)) {
          pending.reject(new Q2QError('NETWORK_PROTOCOL_ERROR'));return;
        }
        if(frame.payload.error) {
          const error=frame.payload.error as {code?:unknown};
          const auth=['expired','bad_signature','bad_mac','unknown_binding','binding_expired','wrong_audience','unauthorized'];
          pending.reject(new Q2QError(typeof error.code==='string'&&auth.includes(error.code)?'AUTH_REQUIRED':'PUNCH_UNAVAILABLE'));
        }
        else if(frame.payload.result!==undefined)pending.resolve(frame.payload.result);
        else pending.reject(new Q2QError('NETWORK_PROTOCOL_ERROR'));
      } else if(frame.type==='signal') {
        if(this.signals.size>=8)throw new Q2QError('RESOURCE_LIMIT');
        const work=Promise.resolve().then(()=>this.onSignal(Object.freeze(frame.payload) as SignalMessage));
        this.signals.add(work);
        void work.catch(()=>this.fail(new Q2QError('NETWORK_PROTOCOL_ERROR'))).finally(()=>this.signals.delete(work));
      } else throw new Q2QError('NETWORK_PROTOCOL_ERROR');
    }catch(error){this.fail(error instanceof Q2QError?error:new Q2QError('NETWORK_PROTOCOL_ERROR'));}
  }
  rpc(method:'punch.join'|'punch.heartbeat'|'punch.exit'|'punch.signal',params:string,signal:AbortSignal,timeoutMs=10000):Promise<unknown> {
    if(signal.aborted)return Promise.reject(cancelled());
    if(this.ended||this.socket.readyState!==WebSocket.OPEN)return Promise.reject(new Q2QError('NETWORK_CLOSED'));
    if(this.pending.size>=32)return Promise.reject(new Q2QError('RESOURCE_LIMIT'));
    const id=crypto.randomUUID();
    const frame=new TextEncoder().encode(`{"type":"request","payload":{"v":1,"id":${JSON.stringify(id)},"method":${JSON.stringify(method)},"params":${params}}}`);
    if(frame.length>LIMIT||this.socket.bufferedAmount+frame.length>256*1024)return Promise.reject(new Q2QError('RESOURCE_LIMIT'));
    return new Promise((resolve,reject)=>{
      const clean=()=>{clearTimeout(timer);signal.removeEventListener('abort',abort);};
      const stop=(error:Q2QError)=>{this.pending.delete(id);clean();reject(error);};
      const abort=()=>stop(cancelled());
      const timer=setTimeout(()=>stop(new Q2QError('CONTROL_TIMEOUT',true)),timeoutMs);
      this.pending.set(id,{resolve,reject,clean});signal.addEventListener('abort',abort,{once:true});
      if(signal.aborted){abort();return;}
      try{this.socket.send(frame);}catch{stop(new Q2QError('NETWORK_UNAVAILABLE',true));}
    });
  }
  close():Promise<void> {
    if(this.closing)return this.closing;
    this.ended=true;
    for(const pending of this.pending.values()){pending.clean();pending.reject(new Q2QError('NETWORK_CLOSED'));}this.pending.clear();
    this.closing=(async()=>{
      await new Promise<void>(resolve=>{
        if(this.socket.readyState===WebSocket.CLOSED){resolve();return;}
        const timer=setTimeout(done,1000);
        const socket=this.socket;
        function done(){clearTimeout(timer);socket.removeEventListener('close',done);resolve();}
        socket.addEventListener('close',done,{once:true});
        try{socket.close(1000);}catch{done();}
      });
      await Promise.allSettled([...this.signals]);
    })();return this.closing;
  }
}
