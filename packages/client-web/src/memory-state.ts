import { StorageError } from './error.ts';
import { validateBinding, validateBitmap, blockCount, encodeCheckpoint, decodeCheckpoint, copyWitnesses, hasBlock } from './checkpoint-codec.ts';
import type { Checkpoint, CheckpointLease, StateStore } from './checkpoint-types.ts';

/** Default current-page state. No global namespace or cross-refresh promise. */
export function memoryState(): StateStore {
  const records = new Map<string, Checkpoint>();
  const witnesses=new Map<string,Map<number,Uint8Array>>();
  const leases = new Map<string, CheckpointLease>();
  let closed = false;
  const copy = (value: Checkpoint) => decodeCheckpoint(encodeCheckpoint('memory', value), 'memory');
  const check = () => { if (closed) throw new StorageError('STATE_CLOSED'); };
  const read = (id: string) => { const value = records.get(id); if (!value) throw new StorageError('CHECKPOINT_NOT_FOUND'); return copy(value); };
  return {
    async create(binding) {
      check(); validateBinding(binding);
      const value = { ...binding, content: { ...binding.content }, id: crypto.randomUUID(), revision: 1,
        committed: new Uint8Array(Math.ceil(blockCount(binding) / 8)), committedBytes: 0n, updatedAt: Date.now() };
      records.set(value.id, copy(value)); return copy(value);
    },
    async read(id) { check(); return read(id); },
    async list(options = {}) {
      check(); const limit = options.limit ?? 32;
      if (!Number.isSafeInteger(limit) || limit < 1 || limit > 128) throw new StorageError('INVALID_RANGE');
      const ids = [...records.keys()].sort().filter(id => options.after === undefined || id > options.after);
      return { items: Object.freeze(ids.slice(0, limit).map(read)), next: ids.length > limit ? ids[limit - 1]! : null };
    },
    async lease(id) {
      check(); if (leases.has(id)) throw new StorageError('CHECKPOINT_BUSY');
      let snapshot = read(id); let ended = false;
      const controller = new AbortController();
      const lease: CheckpointLease = {
        get closed() { return ended; }, signal: controller.signal,
        snapshot: () => copy(snapshot),
        async witness(index){
          check();if(ended)throw new StorageError('INVALID_STATE');
          if(!Number.isSafeInteger(index)||index<0||index>=blockCount(snapshot))throw new StorageError('INVALID_RANGE');
          return witnesses.get(id)?.get(index)?.slice()??null;
        },
        async replace(revision, committed, changes) {
          check(); if (ended) throw new StorageError('INVALID_STATE');
          const current = read(id);
          if (current.revision !== revision) throw new StorageError('CHECKPOINT_CONFLICT');
          if (revision === Number.MAX_SAFE_INTEGER) throw new StorageError('RESOURCE_LIMIT');
          validateBitmap(committed, blockCount(current));
          const updates=copyWitnesses(changes,blockCount(current),committed);
          const nextWitnesses=new Map(witnesses.get(id));
          for(const [index,bytes]of updates)nextWitnesses.set(index,bytes);
          for(const index of nextWitnesses.keys())if(!hasBlock(committed,index))nextWitnesses.delete(index);
          const next = copy({ ...current, revision: revision + 1, committed: new Uint8Array(committed), updatedAt: Date.now() });
          const stored=copy(next);witnesses.set(id,nextWitnesses);records.set(id,stored);snapshot=next;return copy(snapshot);
        },
        async close() { if (!ended) { ended = true; leases.delete(id); controller.abort(); } }
      };
      leases.set(id, lease); return lease;
    },
    async discard(id, revision) {
      check(); if (leases.has(id)) throw new StorageError('CHECKPOINT_BUSY');
      if (revision !== undefined && read(id).revision !== revision) throw new StorageError('CHECKPOINT_CONFLICT');
      witnesses.delete(id);
      if (!records.delete(id)) throw new StorageError('CHECKPOINT_NOT_FOUND');
    },
    async close() { if (!closed) { closed = true; await Promise.all([...leases.values()].map(lease => lease.close())); records.clear();witnesses.clear(); } }
  };
}
