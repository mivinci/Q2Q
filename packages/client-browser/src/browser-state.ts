import { StorageError, storageFailure } from './error.ts';
import { blockCount, decodeCheckpoint, encodeCheckpoint, UUID, validateBinding, validateBitmap, copyWitnesses, decodeWitness, hasBlock, MAX_BLOCKS } from './checkpoint-codec.ts';
import type { Checkpoint, CheckpointLease, StateStore } from './checkpoint-types.ts';

const DATABASE = 'q2q-checkpoints-v1';

/** Versioned metadata only: no file bytes, bearer URLs, seed sources or secrets. */
export async function browserState(options: { namespace: string }): Promise<StateStore> {
  const namespace = options.namespace;
  if (typeof namespace !== 'string' || !/^[a-zA-Z0-9._-]{1,64}$/.test(namespace)) throw new StorageError('INVALID_RANGE');
  if (!globalThis.indexedDB || !globalThis.navigator?.locks) throw new StorageError('UNSUPPORTED_CAPABILITY');
  const db = await new Promise<IDBDatabase>((resolve, reject) => {
    const request = indexedDB.open(DATABASE, 2);
    request.onupgradeneeded = () => {
      if(!request.result.objectStoreNames.contains('checkpoints'))request.result.createObjectStore('checkpoints',{keyPath:['namespace','id']});
      if(!request.result.objectStoreNames.contains('witnesses'))request.result.createObjectStore('witnesses',{keyPath:['namespace','id','index']});
    };
    request.onerror = () => reject(request.error?.name === 'VersionError'
      ? new StorageError('CHECKPOINT_VERSION') : storageFailure(request.error));
    request.onsuccess = () => resolve(request.result);
  });
  let closed = false;
  let closing: Promise<void> | undefined;
  const pending = new Set<Promise<unknown>>();
  const leases = new Set<CheckpointLease>();
  function available() { if (closed) throw new StorageError('STATE_CLOSED'); }
  function validateId(id: string) { if (!UUID.test(id)) throw new StorageError('INVALID_RANGE'); }
  function track<T>(operation: () => Promise<T>): Promise<T> {
    try {
      available();
      const result = operation().catch(error => { throw storageFailure(error); });
      pending.add(result);
      void result.then(() => pending.delete(result), () => pending.delete(result));
      return result;
    } catch (error) { return Promise.reject(storageFailure(error)); }
  }
  function transaction<T>(mode: IDBTransactionMode, run: (store: IDBObjectStore, result: (value: T) => void) => void): Promise<T> {
    return new Promise((resolve, reject) => {
      const tx = db.transaction(['checkpoints','witnesses'], mode);
      let result: T;
      let failure: unknown;
      const fail = (error: unknown) => { failure = error; tx.abort(); };
      tx.oncomplete = () => resolve(result);
      tx.onabort = () => reject(failure ?? storageFailure(tx.error));
      tx.onerror = () => {};
      try {
        const store = tx.objectStore('checkpoints');
        // All callbacks below execute synchronously within IDB request events.
        run(store, value => { result = value; });
        store.transaction.addEventListener('error', event => { failure ??= storageFailure((event.target as IDBRequest).error??tx.error); });
      } catch (error) { fail(error); }
      // Request handlers use this wrapper to route validation errors to abort.
      guarded.set(tx, fail);
    });
  }
  const guarded = new WeakMap<IDBTransaction, (error: unknown) => void>();
  function onResult(request: IDBRequest, run: () => void) {
    request.onsuccess = () => {
      try { run(); } catch (error) { guarded.get(request.transaction!)!(error); }
    };
  }
  function read(id: string) {
    return transaction<Checkpoint>('readonly', (store, done) => {
      const request = store.get([namespace, id]);
      onResult(request, () => done(decodeCheckpoint(request.result, namespace, id)));
    });
  }
  async function exclusive(id: string): Promise<() => Promise<void>> {
    let release!: () => void;
    const held = new Promise<void>(resolve => { release = resolve; });
    let success!: () => void;
    let failure!: (error: unknown) => void;
    const ready = new Promise<void>((resolve, reject) => { success = resolve; failure = reject; });
    const request = navigator.locks.request(`q2q:checkpoint:${namespace}:${id}`, { ifAvailable: true }, async lock => {
      if (!lock) throw new StorageError('CHECKPOINT_BUSY');
      success();
      await held;
    });
    void request.catch(failure);
    await ready;
    return async () => { release(); await request; };
  }
  const state: StateStore = {
    create(binding) {
      return track(async () => {
        validateBinding(binding);
        const record = { ...binding, content: { ...binding.content }, id: crypto.randomUUID(), revision: 1,
          committed: new Uint8Array(Math.ceil(blockCount(binding) / 8)), committedBytes: 0n, updatedAt: Date.now() };
        return transaction('readwrite', (store, done) => {
          store.add(encodeCheckpoint(namespace, record));
          done(decodeCheckpoint(encodeCheckpoint(namespace, record), namespace));
        });
      });
    },
    read(id) { return track(async () => { validateId(id); return read(id); }); },
    list(settings = {}) {
      return track(async () => {
        const limit = settings.limit ?? 32;
        const after = settings.after;
        if (!Number.isSafeInteger(limit) || limit < 1 || limit > 128) throw new StorageError('INVALID_RANGE');
        if (after !== undefined) validateId(after);
        return transaction('readonly', (store, done) => {
          const range = IDBKeyRange.bound([namespace, after ?? ''], [namespace, '\uffff'], after !== undefined);
          const request = store.getAll(range, limit + 1);
          onResult(request, () => {
            const items: Checkpoint[] = request.result.slice(0, limit).map((value: unknown) => decodeCheckpoint(value, namespace));
            done({ items: Object.freeze(items), next: request.result.length > limit ? items[items.length - 1]!.id : null });
          });
        });
      });
    },
    lease(id) {
      return track(async () => {
        validateId(id);
        const unlock = await exclusive(id);
        try {
          available();
          let snapshot = await read(id);
          available();
          let ended = false;
          const cancellation = new AbortController();
          let tail: Promise<void> = Promise.resolve();
          let ending: Promise<void> | undefined;
          const lease: CheckpointLease = {
            get closed() { return ended; },
            signal: cancellation.signal,
            snapshot() { return decodeCheckpoint(encodeCheckpoint(namespace, snapshot), namespace); },
            witness(index){
              const result=track(()=>tail.then(()=>{
                available();if(ended)throw new StorageError('INVALID_STATE');
                if(!Number.isSafeInteger(index)||index<0||index>=blockCount(snapshot))throw new StorageError('INVALID_RANGE');
                return transaction<Uint8Array|null>('readonly',(store,done)=>{
                  const request=store.transaction.objectStore('witnesses').get([namespace,id,index]);
                  onResult(request,()=>done(decodeWitness(request.result,namespace,id,index)));
                });
              }));tail=result.then(()=>undefined,()=>undefined);return result;
            },
            replace(expectedRevision, committed, witnesses) {
              let copy: Uint8Array;let updates:Map<number,Uint8Array>;
              try {
                available();
                if (ended) throw new StorageError('INVALID_STATE');
                if (!Number.isSafeInteger(expectedRevision) || expectedRevision < 1) throw new StorageError('INVALID_RANGE');
                validateBitmap(committed, blockCount(snapshot));
                copy = new Uint8Array(committed);updates=copyWitnesses(witnesses,blockCount(snapshot),copy);
              } catch (error) { return Promise.reject(storageFailure(error)); }
              const result = track(() => tail.then(() => {
                available();
                if (ended) throw new StorageError('INVALID_STATE');
                return transaction<Checkpoint>('readwrite', (store, done) => {
                  const request = store.get([namespace, id]);
                  onResult(request, () => {
                    const current = decodeCheckpoint(request.result, namespace, id);
                    if (current.content.id !== snapshot.content.id || current.targetId !== snapshot.targetId
                      || current.content.size !== snapshot.content.size || current.content.blockSize !== snapshot.content.blockSize) {
                      throw new StorageError('CHECKPOINT_MISMATCH');
                    }
                    if (current.revision !== expectedRevision) throw new StorageError('CHECKPOINT_CONFLICT');
                    if (current.revision === Number.MAX_SAFE_INTEGER) throw new StorageError('RESOURCE_LIMIT');
                    const next = { ...current, revision: current.revision + 1, committed: copy, updatedAt: Date.now() };
                    const record = encodeCheckpoint(namespace, next);
                    const proofs=store.transaction.objectStore('witnesses');
                    for(const [index,bytes]of updates)proofs.put({namespace,id,index,version:1,bytes});
                    for(let index=0;index<blockCount(current);index++)if(hasBlock(current.committed,index)&&!hasBlock(copy,index))proofs.delete([namespace,id,index]);
                    store.put(record);
                    done(decodeCheckpoint(record, namespace, id));
                  });
                }).then(next => { snapshot = next; return decodeCheckpoint(encodeCheckpoint(namespace, snapshot), namespace); });
              }));
              tail = result.then(() => undefined, () => undefined);
              return result;
            },
            close() {
              if (!ending) {
                ended = true;
                ending = tail.then(unlock).finally(() => { leases.delete(lease); });
                cancellation.abort();
              }
              return ending;
            }
          };
          leases.add(lease);
          return lease;
        } catch (error) { await unlock(); throw error; }
      });
    },
    discard(id, expectedRevision) {
      return track(async () => {
        validateId(id);
        const unlock = await exclusive(id);
        try {
          available();
          await transaction<void>('readwrite', (store, done) => {
            const request = store.get([namespace, id]);
            onResult(request, () => {
              if (!request.result) throw new StorageError('CHECKPOINT_NOT_FOUND');
              if (expectedRevision !== undefined && decodeCheckpoint(request.result, namespace, id).revision !== expectedRevision) {
                throw new StorageError('CHECKPOINT_CONFLICT');
              }
              store.delete([namespace, id]);
              store.transaction.objectStore('witnesses').delete(IDBKeyRange.bound([namespace,id,0],[namespace,id,MAX_BLOCKS]));
              done(undefined);
            });
          });
        } finally { await unlock(); }
      });
    },
    close() {
      if (!closing) {
        closed = true;
        closing = (async () => {
          await Promise.allSettled([...leases].map(lease => lease.close()));
          await Promise.allSettled([...pending]);
          db.close();
        })();
      }
      return closing;
    }
  };
  db.onversionchange = () => { void state.close(); };
  return Object.freeze(state);
}
