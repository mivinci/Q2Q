import { StorageError, storageFailure } from './error.ts';
import { blockCount, hasBlock, setBlock } from './checkpoint-codec.ts';
import type { ContentVerifier } from './block-verifier.ts';
import type { Checkpoint, StateStore } from './checkpoint-types.ts';
import type { ContentDestination, DestinationWriter, OutputReceipt } from './storage.ts';

export interface CheckpointSession {
  snapshot(): Checkpoint;
  readCommittedBlock(index:bigint):Promise<{bytes:Uint8Array;proof:string}|null>;
  writeBlock(index: bigint, bytes: Uint8Array, witness?:Uint8Array): Promise<void>;
  /** Flush target bytes first, then CAS the verified-block bitmap. */
  commit(): Promise<Checkpoint>;
  finish(): Promise<OutputReceipt>;
  abort(): Promise<void>;
}

/** A storage session, not a network downloader. The caller owns state/verifier. */
export async function resumeCheckpoint(options: {
  state: StateStore; checkpointId: string; verifier: ContentVerifier;
  destination: ContentDestination; signal?: AbortSignal; maxPendingBytes?: number; mode?: 'create' | 'resume';
}): Promise<CheckpointSession> {
  const { state: store, verifier, destination, signal } = options;
  const budget = options.maxPendingBytes ?? 8 * 1024 * 1024;
  if (!Number.isSafeInteger(budget) || budget < verifier.content.blockSize) throw new StorageError('INVALID_RANGE');
  if (signal?.aborted) throw new StorageError('TASK_CANCELLED');
  const lease = await store.lease(options.checkpointId);
  let writer: DestinationWriter;
  let snapshot = lease.snapshot();
  const stopIO = () => { void writer?.abort().catch(() => {}); };
  try {
    if (snapshot.content.id !== verifier.content.id || snapshot.content.size !== verifier.content.size
      || snapshot.content.blockSize !== verifier.content.blockSize || snapshot.targetId !== destination.targetId) {
      throw new StorageError('CHECKPOINT_MISMATCH');
    }
    if(verifier.requiresWitness&&!lease.witness)throw new StorageError('UNSUPPORTED_CAPABILITY');
    writer = await destination.open({ mode: options.mode ?? 'resume', size: snapshot.content.size, signal });
    lease.signal.addEventListener('abort', stopIO, { once: true });
    if (lease.closed) { await writer.abort(); throw new StorageError('STATE_CLOSED'); }
    try {
      const repaired = new Uint8Array(snapshot.committed.length);
      let revalidated = false;
      for (let index = 0; index < verifier.blockCount; index++) {
        if (lease.closed) throw new StorageError('STATE_CLOSED');
        if (signal?.aborted) throw new StorageError('TASK_CANCELLED');
        if (!hasBlock(snapshot.committed, index)) continue;
        const offset = BigInt(index * snapshot.content.blockSize);
        const remaining = snapshot.content.size - offset;
        const length = Number(remaining < BigInt(snapshot.content.blockSize) ? remaining : BigInt(snapshot.content.blockSize));
        const bytes = await writer.read(offset, length);
        const witness=verifier.requiresWitness?await lease.witness!(index):undefined;
        if (bytes !== null && verifier.verify(BigInt(index), bytes,witness??undefined)) {
          // User-file writable streams can own a private copy made at open.
          // Pin the exact verified bytes into this writer, not just its bitmap.
          await writer.write(offset, bytes);
          setBlock(repaired, index);
          revalidated = true;
        }
      }
      if (signal?.aborted) throw new StorageError('TASK_CANCELLED');
      if (revalidated) await writer.commit();
      if (signal?.aborted) throw new StorageError('TASK_CANCELLED');
      // Every recovery increments the generation, even if no blocks changed.
      snapshot = await lease.replace(snapshot.revision, repaired);
    } catch (error) { try { await writer.abort(); } catch { /* preserve recovery failure */ } throw error; }
  } catch (error) { lease.signal.removeEventListener('abort', stopIO); await lease.close(); throw storageFailure(error); }

  let tail: Promise<void> = Promise.resolve();
  let status: 'active' | 'finishing' | 'finished' | 'aborting' | 'aborted' | 'failed' = 'active';
  let failure: StorageError | undefined;
  let dirty = new Uint8Array(snapshot.committed.length);
  let queuedBytes = 0;
  let dirtyWitnesses=new Map<number,Uint8Array>();
  let finishing: Promise<OutputReceipt> | undefined;
  let aborting: Promise<void> | undefined;
  async function cleanup() {
    try { await writer.abort(); }
    finally { lease.signal.removeEventListener('abort', stopIO); await lease.close(); }
  }
  function check() {
    if (failure) throw failure;
    if (status === 'aborting' || status === 'aborted' || signal?.aborted) throw new StorageError('TASK_CANCELLED');
    if (lease.closed) throw new StorageError('STATE_CLOSED');
  }
  function active() {
    check();
    if (status !== 'active') throw new StorageError('INVALID_STATE');
  }
  function enqueue<T>(operation: () => Promise<T>): Promise<T> {
    const result = tail.then(async () => {
      try { check(); return await operation(); }
      catch (error) {
        const mapped = storageFailure(error);
        if (status !== 'aborting') {
          status = 'failed'; failure = mapped;
          try { await cleanup(); } catch { /* keep original failure */ }
        }
        throw mapped;
      }
    });
    tail = result.then(() => undefined, () => undefined);
    return result;
  }
  async function commit() {
    await writer.commit();
    check();
    const bitmap = new Uint8Array(snapshot.committed);
    for (let i = 0; i < bitmap.length; i++) bitmap[i]! |= dirty[i]!;
    snapshot = await lease.replace(snapshot.revision, bitmap,dirtyWitnesses);
    dirty = new Uint8Array(bitmap.length);dirtyWitnesses=new Map();
    check();
    return lease.snapshot();
  }
  const session: CheckpointSession = {
    snapshot: () => lease.snapshot(),
    readCommittedBlock(index){
      try{
        active();
        if(typeof index!=='bigint'||index<0n||index>=BigInt(verifier.blockCount))throw new StorageError('INVALID_RANGE');
        if(!verifier.proof)throw new StorageError('UNSUPPORTED_CAPABILITY');
        return enqueue(async()=>{
          if(!hasBlock(snapshot.committed,Number(index)))return null;
          const offset=index*BigInt(snapshot.content.blockSize),remaining=snapshot.content.size-offset;
          const bytes=await writer.read(offset,Number(remaining<BigInt(snapshot.content.blockSize)?remaining:BigInt(snapshot.content.blockSize)));
          const witness=verifier.requiresWitness?await lease.witness!(Number(index)):undefined;
          if(bytes===null||!verifier.verify(index,bytes,witness??undefined))throw new StorageError('CONTENT_MISMATCH');
          return {bytes,proof:verifier.proof!(index,bytes,witness??undefined)};
        });
      }catch(error){return Promise.reject(error);}
    },
    writeBlock(index, bytes, witness) {
      try {
        active();
        if (typeof index !== 'bigint' || index < 0n || index >= BigInt(verifier.blockCount)) throw new StorageError('INVALID_RANGE');
        if (bytes.byteLength > budget - queuedBytes) throw new StorageError('RESOURCE_LIMIT');
        const copy = new Uint8Array(bytes);
        // Reject bad content without ending a usable session: another origin may
        // supply the correct block. Never persist a claimed-valid bit for it.
        const proof=verifier.requiresWitness?witness?.slice():undefined;
        if(proof&&proof.length>640)throw new StorageError('RESOURCE_LIMIT');
        if(verifier.requiresWitness&&!dirtyWitnesses.has(Number(index))&&dirtyWitnesses.size>=1024)throw new StorageError('RESOURCE_LIMIT');
        if (!verifier.verify(index, copy,proof)) throw new StorageError('CONTENT_MISMATCH');
        queuedBytes += copy.length;
        const result = enqueue(async () => {
          await writer.write(index * BigInt(snapshot.content.blockSize), copy);
          check();
          setBlock(dirty, Number(index));if(proof)dirtyWitnesses.set(Number(index),proof);
        });
        return result.finally(() => { queuedBytes -= copy.length; });
      } catch (error) { return Promise.reject(storageFailure(error)); }
    },
    commit() {
      try { active(); return enqueue(commit); } catch (error) { return Promise.reject(error); }
    },
    finish() {
      if (finishing) return finishing;
      try { active(); } catch (error) { return Promise.reject(error); }
      status = 'finishing';
      finishing = enqueue(async () => {
        for (let index = 0; index < blockCount(snapshot); index++) {
          if (!hasBlock(snapshot.committed, index) && !hasBlock(dirty, index)) throw new StorageError('INVALID_STATE');
        }
        await commit();
        check();
        const output = await writer.finish();
        // Cancellation during an already-started native finalization may leave
        // the bytes saved. It still must not report successful task completion.
        check();
        status = 'finished';
        lease.signal.removeEventListener('abort', stopIO);
        await lease.close();
        return output;
      });
      return finishing;
    },
    abort() {
      if (aborting) return aborting;
      if (status === 'finished') return Promise.resolve();
      status = 'aborting';
      aborting = tail.then(async () => {
        try { await cleanup(); status = 'aborted'; }
        catch (error) { status = 'failed'; throw storageFailure(error); }
      });
      return aborting;
    }
  };
  return Object.freeze(session);
}
