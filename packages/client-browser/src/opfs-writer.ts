import { StorageError, storageFailure } from './error.ts';
import type { DestinationWriter, OutputReceipt } from './storage.ts';
import type { OpfsCommand, OpfsResponse } from './opfs-protocol.ts';

interface WorkerWriterOptions {
  path: readonly string[];
  targetId: string;
  size: bigint;
  maxPendingBytes: number;
  workerUrl?: string | URL;
  signal?: AbortSignal;
  complete(): Promise<void>;
  release(): Promise<void>;
}

/** One worker owns one sync handle; no per-block file close or whole-file copy. */
export async function opfsWriter(options: WorkerWriterOptions): Promise<DestinationWriter> {
  let worker: Worker;
  try {
    worker = options.workerUrl
      ? new Worker(options.workerUrl, { type: 'module' })
      : new Worker(new URL('./opfs-worker.ts', import.meta.url), { type: 'module' });
  } catch (error) { await options.release(); throw storageFailure(error); }
  let id = 0;
  let crashed = false;
  const pending = new Map<number, { resolve(value: OpfsResponse): void; reject(error: unknown): void }>();
  const ready = new Promise<OpfsResponse>((resolve, reject) => { pending.set(0, { resolve, reject }); });
  function failWorker() {
    crashed = true;
    worker.terminate();
    for (const wait of pending.values()) wait.reject(new StorageError('STORAGE_UNAVAILABLE'));
    pending.clear();
    // A worker can fail while idle too; do not wait for another caller command
    // before releasing its SDK lease. Cleanup is safe after native termination.
    void cleanup().catch(() => {});
  }
  worker.onerror = event => { event.preventDefault(); failWorker(); };
  worker.onmessageerror = failWorker;
  worker.onmessage = ({ data }: MessageEvent<OpfsResponse>) => {
    const wait = pending.get(data.id);
    if (!wait) return;
    pending.delete(data.id);
    if (data.error) wait.reject(new StorageError(data.error));
    else wait.resolve(data);
  };
  function rpc(command: OpfsCommand): Promise<OpfsResponse> {
    if (crashed) return Promise.reject(new StorageError('STORAGE_UNAVAILABLE'));
    return new Promise((resolve, reject) => {
      const requestId = ++id;
      pending.set(requestId, { resolve, reject });
      try {
        worker.postMessage({ ...command, id: requestId },
          command.type === 'write' ? [command.bytes.buffer] : []);
      } catch (error) { pending.delete(requestId); reject(storageFailure(error)); }
    });
  }
  let opened = false;
  let releasing: Promise<void> | undefined;
  async function cleanup() {
    try { if (opened && !crashed) { await rpc({ type: 'close' }); opened = false; } }
    finally {
      opened = false;
      crashed = true;
      worker.terminate();
      releasing ??= options.release();
      await releasing;
    }
  }
  try {
    // A missing worker script must not hang initialization and hold a target lock.
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      await Promise.race([
        ready.then(() => rpc({ type: 'open', path: options.path })),
        new Promise<never>((_, reject) => {
          timer = setTimeout(() => { failWorker(); reject(new StorageError('STORAGE_UNAVAILABLE')); }, 10000);
        })
      ]);
    } finally { clearTimeout(timer); }
    opened = true;
    if (options.signal?.aborted) throw new StorageError('TASK_CANCELLED');
  } catch (error) { try { await cleanup(); } catch { /* preserve init error */ } throw storageFailure(error); }

  let tail: Promise<void> = Promise.resolve();
  let state: 'active' | 'finishing' | 'finished' | 'aborting' | 'aborted' | 'failed' = 'active';
  let failure: StorageError | undefined;
  let queuedBytes = 0;
  let finishing: Promise<OutputReceipt> | undefined;
  let aborting: Promise<void> | undefined;
  function check() {
    if (failure) throw failure;
    if (state === 'aborting' || state === 'aborted') throw new StorageError('TASK_CANCELLED');
  }
  function active() { if (state !== 'active') throw failure ?? new StorageError('INVALID_STATE'); }
  function enqueue<T>(operation: () => Promise<T>): Promise<T> {
    const result = tail.then(async () => {
      try { check(); return await operation(); }
      catch (error) {
        const mapped = storageFailure(error);
        if (mapped.code !== 'TASK_CANCELLED') {
          failure = mapped;
          state = 'failed';
          try { await cleanup(); } catch { /* original error owns failure */ }
        }
        throw mapped;
      }
    });
    tail = result.then(() => undefined, () => undefined);
    return result;
  }
  return {
    read(offset, length) {
      try {
        active();
        if (typeof offset !== 'bigint' || offset < 0n || offset > options.size
          || !Number.isSafeInteger(length) || length < 0 || length > 4 * 1024 * 1024
          || BigInt(length) > options.size - offset) throw new StorageError('INVALID_RANGE');
        if (length > options.maxPendingBytes - queuedBytes) throw new StorageError('RESOURCE_LIMIT');
        queuedBytes += length;
        const result = enqueue(async () => {
          const response = await rpc({ type: 'read', offset: Number(offset), length });
          check();
          if (response.bytes === undefined) throw new StorageError('STORAGE_UNAVAILABLE');
          return response.bytes;
        });
        return result.finally(() => { queuedBytes -= length; });
      } catch (error) { return Promise.reject(error); }
    },
    write(offset, bytes) {
      try {
        active();
        if (typeof offset !== 'bigint' || offset < 0n || offset > options.size
          || BigInt(bytes.byteLength) > options.size - offset) throw new StorageError('INVALID_RANGE');
        if (bytes.byteLength > options.maxPendingBytes - queuedBytes) throw new StorageError('RESOURCE_LIMIT');
        const copy = new Uint8Array(bytes);
        const length = copy.byteLength;
        queuedBytes += length;
        const result = enqueue(async () => {
          await rpc({ type: 'write', offset: Number(offset), bytes: copy });
          check();
        });
        return result.finally(() => { queuedBytes -= length; });
      } catch (error) { return Promise.reject(error); }
    },
    commit() {
      try { active(); return enqueue(async () => { await rpc({ type: 'flush' }); check(); }); }
      catch (error) { return Promise.reject(error); }
    },
    finish() {
      if (finishing) return finishing;
      try { active(); } catch (error) { return Promise.reject(error); }
      state = 'finishing';
      finishing = enqueue(async () => {
        await rpc({ type: 'flush' });
        check();
        await rpc({ type: 'close' });
        opened = false;
        check();
        // Finalize metadata only after bytes are flushed and the native lock closes.
        await options.complete();
        check();
        await cleanup();
        state = 'finished';
        return Object.freeze({ kind: 'staged' as const, targetId: options.targetId, needsExport: true as const });
      });
      return finishing;
    },
    abort() {
      if (aborting) return aborting;
      if (state === 'finished') return Promise.resolve();
      state = 'aborting';
      aborting = tail.then(async () => {
        try { await cleanup(); state = 'aborted'; }
        catch (error) { state = 'failed'; throw storageFailure(error); }
      });
      return aborting;
    }
  };
}
