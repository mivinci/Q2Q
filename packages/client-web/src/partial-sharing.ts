import {Q2QError} from './error.ts';
import type {CheckpointSession} from './resume-checkpoint.ts';

export interface SharedBlock {bytes:Uint8Array;proof:string}
export interface LiveBlocks {
 readonly signal:AbortSignal;
 ready:Promise<void>;
 bitmap():Uint8Array;
 read(index:number):Promise<SharedBlock|null>;
 subscribe(listener:(index:number)=>void):()=>void;
}
/** The only data source is the current writer's committed-and-reverified view.
 * No raw OPFS source, historic UI bitmap, or uncommitted in-memory block is exposed. */
export function partialSharing(count:number){
 const controller=new AbortController(),listeners=new Set<(index:number)=>void>();
 let session:CheckpointSession|undefined,bitmap=new Uint8Array(Math.ceil(count/8));
 let resolve!:()=>void;const ready=new Promise<void>(done=>{resolve=done;});
 const access:LiveBlocks={signal:controller.signal,ready,
  bitmap:()=>bitmap.slice(),
  async read(index){if(controller.signal.aborted||!session)throw new Q2QError('SOURCE_UNAVAILABLE');return session.readCommittedBlock(BigInt(index));},
  subscribe(listener){listeners.add(listener);return()=>{listeners.delete(listener);};}
 };
 return {access,
  attach(value:CheckpointSession){session=value;bitmap=value.snapshot().committed.slice();resolve();},
  committed(){if(!session||controller.signal.aborted)return;const next=session.snapshot().committed;
   const previous=bitmap;bitmap=next.slice();
   for(let index=0;index<count;index++)if((next[index>>3]!&(1<<(index%8)))&&!(previous[index>>3]!&(1<<(index%8))))for(const listener of listeners){try{listener(index);}catch{}}
  },
  end(){controller.abort();session=undefined;bitmap.fill(0);listeners.clear();resolve();}
 };
}
export type PartialSharing=ReturnType<typeof partialSharing>;
