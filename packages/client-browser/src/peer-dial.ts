import { Q2QError, type ConnectionDiagnostic } from './error.ts';
import { decodeKey } from './control-identity.ts';
import type { ControlIdentity } from './control-identity.ts';
import { PunchSocket } from './punch-socket.ts';
import type { SignalMessage } from './punch-socket.ts';
import { RtcPeer, Inbox } from './rtc-peer.ts';
export interface PeerCandidate {peerId:string;publicKey:Uint8Array;punchId:string;websocketUrl:string;complete:boolean}
export interface PeerConnection {rtc:RtcPeer;close():Promise<void>}

/** One isolated ephemeral Punch connection per dial, matching the native SDK. */
export async function dialPeer(options:{identity:ControlIdentity;target:PeerCandidate;hash:Uint8Array;iceServers:RTCIceServer[];signal:AbortSignal;timeoutMs:number}):Promise<PeerConnection>{
  const controller=new AbortController();const abort=()=>controller.abort();options.signal.addEventListener('abort',abort,{once:true});if(options.signal.aborted)abort();
  let stage:ConnectionDiagnostic['stage']='punch-connect';
  let problem:Q2QError|undefined;let rtc:RtcPeer|undefined;let socket:PunchSocket|undefined;let pump:Promise<void>|undefined;let closing:Promise<void>|undefined;
  const inbox=new Inbox<SignalMessage>();const id=crypto.randomUUID();
  let releaseOffer!:()=>void;const offered=new Promise<void>(resolve=>{releaseOffer=resolve;});
  const deadline=setTimeout(()=>{problem=new Q2QError('CONTROL_TIMEOUT',true);controller.abort();},options.timeoutMs);
  const send=(value:unknown)=>socket!.rpc('punch.signal',JSON.stringify(value),controller.signal,Math.min(options.timeoutMs,10000));
  const stop=()=>{releaseOffer();inbox.close(problem??new Q2QError('TASK_CANCELLED'));void socket?.close();void rtc?.close();};
  controller.signal.addEventListener('abort',stop,{once:true});
  function close():Promise<void>{
    if(closing)return closing;
    clearTimeout(deadline);releaseOffer();controller.abort();options.signal.removeEventListener('abort',abort);
    closing=(async()=>{await Promise.allSettled([socket?.close(),rtc?.close(),pump]);})();return closing;
  }
  try{
    if(options.identity.peerId(options.target.publicKey)!==options.target.peerId)throw new Q2QError('PEER_AUTH_FAILED');
    socket=await PunchSocket.open({url:options.target.websocketUrl,identity:options.identity,signal:controller.signal,timeoutMs:options.timeoutMs,
      onSignal:message=>{if(message.connection_id===id)inbox.push(message,new TextEncoder().encode(JSON.stringify(message)).length);},
      onFailure:error=>{problem=error;inbox.close(error);controller.abort();}});
    const token=options.identity.connectionTicket(options.target.punchId,options.target.peerId,options.hash,id);
    await send({type:'connect',connection_id:id,connect_token:token});
    stage='punch-accept';
    const accept=await inbox.read(controller.signal);
    if(accept.type!=='accept'||typeof accept.accept_token!=='string')throw new Q2QError('PUNCH_UNAVAILABLE',accept.type==='reject');
    options.identity.verifyAccept(accept.accept_token,options.target.publicKey,id,options.target.punchId,options.hash);
    rtc=await RtcPeer.create({identity:options.identity,remoteKey:options.target.publicKey,infoHash:options.hash,connectionId:id,initiator:true,iceServers:options.iceServers,
      signal:controller.signal,timeoutMs:options.timeoutMs,onCandidate:async candidate=>{await offered;if(!controller.signal.aborted)await send({type:'candidate',connection_id:id,candidate:JSON.stringify(candidate)});}});
    stage='rtc-offer';const offer=await rtc.offer();
    stage='punch-offer';await send({type:'offer',connection_id:id,connect_token:token,sdp:JSON.stringify(offer)});releaseOffer();
    stage='punch-answer';pump=(async()=>{
      let answered=false;
      for(;;){
        const message=await inbox.read(controller.signal);
        if(message.type==='answer'&&!answered&&typeof message.sdp==='string'){
          answered=true;stage='rtc-connect';await rtc!.applyAnswer(JSON.parse(message.sdp) as RTCSessionDescriptionInit);
        }else if(message.type==='candidate'&&typeof message.candidate==='string')await rtc!.addCandidate(JSON.parse(message.candidate) as RTCIceCandidateInit);
        else throw new Q2QError('PEER_PROTOCOL_ERROR');
      }
    })();void pump.catch(error=>{if(!controller.signal.aborted){problem=error instanceof Q2QError?error:new Q2QError('PEER_PROTOCOL_ERROR');controller.abort();}});
    await rtc.ready();clearTimeout(deadline);
    return {rtc,close};
  }catch(error){
    // Preserve the RTC failure captured before teardown, even when the outer
    // deadline or signal pump also fires. Never serialize native exception text.
    const cause=problem??(error instanceof Q2QError?error:new Q2QError('PEER_PROTOCOL_ERROR'));
    const rtcDiagnostic=error instanceof Q2QError?error.diagnostic:undefined;
    const failure=cause.diagnostic?cause:new Q2QError(cause.code,cause.retryable,{...rtcDiagnostic,stage:rtcDiagnostic&&!problem?rtcDiagnostic.stage:stage});
    await close();throw failure;
  }
}
