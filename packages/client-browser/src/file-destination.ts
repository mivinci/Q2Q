import { StorageError, storageFailure } from './error.ts';
import { fileSource } from './file-source.ts';
import { identifyFile, leaseFile } from './file-target.ts';
import type { ContentDestination, DestinationWriter, OutputReceipt } from './storage.ts';

export interface FileDestinationOptions {
  /** Application must obtain overwrite consent before enabling this. */
  overwrite?: boolean;
  /** Bound retained write copies, including queued and in-flight operations. */
  maxPendingBytes?: number;
}

/** Takes an already-authorized handle, never requests permissions or opens UI. */
export async function fileDestination(
  handle: FileSystemFileHandle,
  options: FileDestinationOptions = {}
): Promise<ContentDestination> {
  const maxPendingBytes = options.maxPendingBytes ?? 8 * 1024 * 1024;
  if (!Number.isSafeInteger(maxPendingBytes) || maxPendingBytes <= 0) throw new StorageError('INVALID_RANGE');
  const overwrite = options.overwrite === true;
  if (!globalThis.navigator?.storage?.getDirectory) throw new StorageError('UNSUPPORTED_CAPABILITY');
  const targetId = await identifyFile(handle);
  // A caller can pass an OPFS handle too. Do not label a private cache as a
  // user-visible saved file simply because it has the same host interface.
  let staged: boolean;
  try {
    const root = await navigator.storage.getDirectory();
    staged = (await root.resolve(handle)) !== null;
  } catch (error) { throw storageFailure(error); }
  return Object.freeze({
    targetId,
    async open({ mode, size, signal }: Parameters<ContentDestination['open']>[0]): Promise<DestinationWriter> {
      if ((mode !== 'create' && mode !== 'resume') || typeof size !== 'bigint' || size < 0n) {
        throw new StorageError('INVALID_RANGE');
      }
      if (size > BigInt(Number.MAX_SAFE_INTEGER)) throw new StorageError('UNSUPPORTED_CAPABILITY');
      if (signal?.aborted) throw new StorageError('TASK_CANCELLED');
      const release = await leaseFile(targetId);
      let stream: FileSystemWritableFileStream | undefined;
      try {
        const file = await handle.getFile();
        if (mode === 'create' && file.size > 0 && !overwrite) throw new StorageError('DESTINATION_EXISTS');
        if (mode === 'resume' && BigInt(file.size) > size) throw new StorageError('CHECKPOINT_MISMATCH');
        if (signal?.aborted) throw new StorageError('TASK_CANCELLED');
        // Keep original data even for an overwrite until an explicit commit.
        const writableOptions = { keepExistingData: true, mode: 'exclusive' };
        stream = await handle.createWritable(writableOptions);
        if (signal?.aborted) throw new StorageError('TASK_CANCELLED');
        return writer(handle, stream, targetId, staged, Number(size), maxPendingBytes, release);
      } catch (error) {
        if (stream) { try { await stream.abort(); } catch { /* report original failure */ } }
        await release();
        throw storageFailure(error);
      }
    }
  });
}

function writer(
  handle: FileSystemFileHandle,
  initial: FileSystemWritableFileStream,
  targetId: string,
  staged: boolean,
  size: number,
  maxPendingBytes: number,
  release: () => Promise<void>
): DestinationWriter {
  let stream: FileSystemWritableFileStream | undefined = initial;
  let tail: Promise<void> = Promise.resolve();
  let state: 'active' | 'finishing' | 'finished' | 'aborting' | 'aborted' | 'failed' = 'active';
  let failure: StorageError | undefined;
  let queuedBytes = 0;
  let released = false;
  let finishing: Promise<OutputReceipt> | undefined;
  let aborting: Promise<void> | undefined;

  async function unlock() {
    if (!released) { released = true; await release(); }
  }
  async function discard() {
    const current = stream;
    stream = undefined;
    try { if (current) await current.abort(); } finally { await unlock(); }
  }
  async function writable() {
    if (!stream) {
      const options = { keepExistingData: true, mode: 'exclusive' };
      stream = await handle.createWritable(options);
    }
    return stream;
  }
  function check() {
    if (failure) throw failure;
    if (state === 'aborting' || state === 'aborted') throw new StorageError('TASK_CANCELLED');
  }
  function enqueue<T>(operation: () => Promise<T>): Promise<T> {
    const result = tail.then(async () => {
      try { check(); return await operation(); }
      catch (error) {
        // Once a host operation fails, queued commands cannot revive this writer.
        const mapped = storageFailure(error);
        if (mapped.code !== 'TASK_CANCELLED') {
          failure = mapped;
          state = 'failed';
          try { await discard(); } catch { /* preserve the original host error */ }
        }
        throw mapped;
      }
    });
    tail = result.then(() => undefined, () => undefined);
    return result;
  }
  function requireActive() {
    if (state !== 'active') throw failure ?? new StorageError('INVALID_STATE');
  }
  async function commitStream() {
    // Closing is not abortable. Abort must wait if this commit has already begun.
    const current = stream;
    if (!current) return;
    await current.truncate(size);
    check();
    await current.close();
    stream = undefined;
  }
  return {
    read(offset, length) {
      try {
        requireActive();
        if (typeof offset !== 'bigint' || offset < 0n || offset > BigInt(size)
          || !Number.isSafeInteger(length) || length < 0 || length > 4 * 1024 * 1024
          || length > size - Number(offset)) throw new StorageError('INVALID_RANGE');
        return enqueue(async () => {
          const file = await handle.getFile();
          if (Number(offset) + length > file.size) return null;
          const reader = await fileSource(file).open();
          try { const bytes = await reader.read(offset, length); check(); return bytes; }
          finally { await reader.close(); }
        });
      } catch (error) { return Promise.reject(error); }
    },
    write(offset, bytes) {
      try {
        requireActive();
        if (typeof offset !== 'bigint' || offset < 0n || offset > BigInt(size)
          || bytes.byteLength > size - Number(offset)) throw new StorageError('INVALID_RANGE');
        if (bytes.byteLength > maxPendingBytes - queuedBytes) throw new StorageError('RESOURCE_LIMIT');
        // Copy before returning so caller mutation cannot change accepted content.
        const copy = new Uint8Array(bytes);
        queuedBytes += copy.byteLength;
        const result = enqueue(async () => {
          await (await writable()).write({ type: 'write', position: Number(offset), data: copy });
          check();
        });
        return result.finally(() => { queuedBytes -= copy.byteLength; });
      } catch (error) { return Promise.reject(error); }
    },
    commit() {
      try { requireActive(); return enqueue(commitStream); }
      catch (error) { return Promise.reject(error); }
    },
    finish() {
      if (finishing) return finishing;
      try { requireActive(); } catch (error) { return Promise.reject(error); }
      state = 'finishing';
      finishing = enqueue(async () => {
        await commitStream();
        check();
        state = 'finished';
        await unlock();
        return staged
          ? Object.freeze({ kind: 'staged' as const, targetId, needsExport: true as const })
          : Object.freeze({ kind: 'user-file' as const, targetId });
      });
      return finishing;
    },
    abort() {
      if (aborting) return aborting;
      if (state === 'finished') return Promise.resolve();
      state = 'aborting';
      aborting = tail.then(async () => {
        try { await discard(); state = 'aborted'; }
        catch (error) { state = 'failed'; throw storageFailure(error); }
      });
      return aborting;
    }
  };
}
