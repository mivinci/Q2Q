import { StorageError, storageFailure } from './error.ts';
import { MAX_BLOCKS } from './checkpoint-codec.ts';
import type { ContentIdentity } from './checkpoint-types.ts';

export interface ContentVerifier {
  readonly content: ContentIdentity;
  readonly blockCount: number;
  readonly requiresWitness?:boolean;
  verify(index: bigint, bytes: Uint8Array, witness?:Uint8Array): boolean;
  verifyPeer?(index:bigint,bytes:Uint8Array,proofFrame:string):Uint8Array;
  proof?(index:bigint,bytes:Uint8Array,witness?:Uint8Array):string;
  close(): void;
}
let initialization: Promise<typeof import('@q2q/wasm')> | undefined;
export function initializeWasm(wasmUrl?: string | URL) {
  // wasm-bindgen has module-global state: concurrent first calls must not
  // initialize different instances underneath already-created Rust objects.
  initialization ??= (async () => {
    const module = await import('@q2q/wasm');
    await module.default(wasmUrl ? { module_or_path: wasmUrl } : undefined);
    return module;
  })().catch(error => { initialization = undefined; throw storageFailure(error); });
  return initialization;
}

/** Validates the seed and proof once; hashing stays in shared Rust primitives. */
export async function createBlockVerifier(options: {
  seed: Uint8Array; proof?: Uint8Array; wasmUrl?: string | URL;
}): Promise<ContentVerifier> {
  if (options.seed.length > 17 * 1024 * 1024 || (options.proof?.length ?? 0) > 16 * 1024 * 1024) {
    throw new StorageError('RESOURCE_LIMIT');
  }
  const seed = new Uint8Array(options.seed);
  const proof = options.proof ? new Uint8Array(options.proof) : new Uint8Array();
  const module = await initializeWasm(options.wasmUrl);
  try {
    const info = JSON.parse(module.decode_seed(seed)) as { infoHash: string; size: string; blockSize: number; blockCount: string };
    if (BigInt(info.size) > BigInt(Number.MAX_SAFE_INTEGER)) throw new StorageError('UNSUPPORTED_CAPABILITY');
    if (BigInt(info.blockCount) > BigInt(MAX_BLOCKS)) throw new StorageError('RESOURCE_LIMIT');
    const verifier = new module.BlockVerifier(seed, proof);
    let closed = false;
    const content = Object.freeze({ id: `sha256:${info.infoHash}` as const, size: BigInt(info.size), blockSize: info.blockSize });
    return Object.freeze({
      content, blockCount: Number(info.blockCount),
      verify(index: bigint, bytes: Uint8Array) {
        if (closed) throw new StorageError('INVALID_STATE');
        if (typeof index !== 'bigint' || index < 0n || index >= BigInt(info.blockCount)) return false;
        return verifier.verify(Number(index), bytes);
      },
      proof(index:bigint,bytes:Uint8Array){
        if(closed||index<0n||index>=BigInt(info.blockCount)||!verifier.verify(Number(index),bytes))throw new StorageError('CONTENT_MISMATCH');
        return verifier.proof_frame(Number(index));
      },
      close() { if (!closed) { closed = true; verifier.free(); } }
    });
  } catch (error) {
    if (error instanceof StorageError) throw error;
    throw new StorageError('INVALID_SEED');
  }
}


/** A sparse verifier never trusts persisted proof flags or cached leaf hashes. */
export async function createPeerBlockVerifier(options:{seed:Uint8Array;wasmUrl?:string|URL}):Promise<ContentVerifier>{
  if(options.seed.length>17*1024*1024)throw new StorageError('RESOURCE_LIMIT');
  const module=await initializeWasm(options.wasmUrl);
  const info=JSON.parse(module.decode_seed(options.seed)) as {infoHash:string;size:string;blockSize:number;blockCount:string};
  if(BigInt(info.blockCount)>65536n)throw new StorageError('UNSUPPORTED_CAPABILITY');
  const verifier=new module.PeerProofVerifier(options.seed);let closed=false;
  const valid=(index:bigint)=>typeof index==='bigint'&&index>=0n&&index<BigInt(info.blockCount);
  const check=()=>{if(closed)throw new StorageError('INVALID_STATE');};
  return Object.freeze({
    content:Object.freeze({id:`sha256:${info.infoHash}` as const,size:BigInt(info.size),blockSize:info.blockSize}),blockCount:Number(info.blockCount),requiresWitness:true,
    verify(index:bigint,bytes:Uint8Array,witness?:Uint8Array){check();return valid(index)&&witness!==undefined&&verifier.verify(Number(index),bytes,witness);},
    verifyPeer(index:bigint,bytes:Uint8Array,raw:string){check();if(!valid(index))throw new StorageError('CONTENT_MISMATCH');try{return verifier.witness(Number(index),bytes,raw);}catch{throw new StorageError('CONTENT_MISMATCH');}},
    proof(index:bigint,bytes:Uint8Array,witness?:Uint8Array){check();if(!valid(index)||!witness)throw new StorageError('CONTENT_MISMATCH');try{return verifier.proof_frame(Number(index),bytes,witness);}catch{throw new StorageError('CONTENT_MISMATCH');}},
    close(){if(!closed){closed=true;verifier.free();}}
  });
}
