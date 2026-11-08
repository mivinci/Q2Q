import { StorageError, storageFailure } from './error.ts';
import { fileSource } from './file-source.ts';
import { identifyFile, leaseFile, forgetFile } from './file-target.ts';
import { opfsWriter } from './opfs-writer.ts';
import type { ContentDestination, ContentSource, OutputReceipt, SourceReader } from './storage.ts';

const DIRECTORY = 'q2q-cache-v1';
const ID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
interface Metadata { version: 1; id: string; name: string; size: string; state: 'partial' | 'complete' }

export interface OpfsCacheOptions {
  workerUrl?: string | URL;
  maxPendingBytes?: number;
}
export interface ExportOptions {
  signal?: AbortSignal;
  chunkBytes?: number;
  onProgress?: (progress: Readonly<{ copiedBytes: bigint; totalBytes: bigint }>) => void;
}
export interface OpfsEntry {
  readonly id: string;
  readonly targetId: string;
  readonly name: string;
  readonly size: bigint;
  readonly destination: ContentDestination;
  /** Only completed entries may be opened. The reader owns a target lease. */
  readonly source: ContentSource;
  status(): Promise<'partial' | 'complete'>;
  /** Streaming copy; it never deletes this cache, even on success. */
  exportTo(destination: ContentDestination, options?: ExportOptions): Promise<OutputReceipt>;
}
export interface OpfsCache {
  create(options: { name: string; size: bigint }): Promise<OpfsEntry>;
  open(id: string): Promise<OpfsEntry>;
  remove(id: string): Promise<void>;
}

function cacheFailure(error: unknown): StorageError {
  if (error instanceof Error && error.name === 'NotFoundError') return new StorageError('CACHE_NOT_FOUND');
  return storageFailure(error);
}
function validateSize(size: bigint) {
  if (typeof size !== 'bigint' || size < 0n) throw new StorageError('INVALID_RANGE');
  if (size > BigInt(Number.MAX_SAFE_INTEGER)) throw new StorageError('UNSUPPORTED_CAPABILITY');
}
async function readMetadata(directory: FileSystemDirectoryHandle, id: string): Promise<Metadata> {
  const file = await (await directory.getFileHandle('metadata.json')).getFile();
  if (file.size > 16384) throw new StorageError('CACHE_CORRUPT');
  let value: Metadata;
  try { value = JSON.parse(await file.text()); } catch { throw new StorageError('CACHE_CORRUPT'); }
  if (!value || value.version !== 1 || value.id !== id || typeof value.name !== 'string'
    || value.name.length > 1024 || typeof value.size !== 'string' || !/^(0|[1-9][0-9]*)$/.test(value.size)
    || value.size.length > 16 || BigInt(value.size) > BigInt(Number.MAX_SAFE_INTEGER)
    || (value.state !== 'partial' && value.state !== 'complete')) throw new StorageError('CACHE_CORRUPT');
  return value;
}
async function writeMetadata(directory: FileSystemDirectoryHandle, metadata: Metadata) {
  const handle = await directory.getFileHandle('metadata.json', { create: true });
  const stream = await handle.createWritable();
  try { await stream.write(JSON.stringify(metadata)); await stream.close(); }
  catch (error) { try { await stream.abort(); } catch { /* preserve failure */ } throw error; }
}

/** Opens only SDK-managed private storage; no user file selection or account required. */
export async function opfsCache(options: OpfsCacheOptions = {}): Promise<OpfsCache> {
  if (!globalThis.navigator?.storage?.getDirectory || !navigator.locks || !globalThis.indexedDB
    || !globalThis.Worker) throw new StorageError('UNSUPPORTED_CAPABILITY');
  const maxPendingBytes = options.maxPendingBytes ?? 8 * 1024 * 1024;
  if (!Number.isSafeInteger(maxPendingBytes) || maxPendingBytes <= 0) throw new StorageError('INVALID_RANGE');
  // Capture options so caller mutation cannot change an existing cache's budget.
  const workerUrl = options.workerUrl;
  let root: FileSystemDirectoryHandle;
  try { root = await (await navigator.storage.getDirectory()).getDirectoryHandle(DIRECTORY, { create: true }); }
  catch (error) { throw cacheFailure(error); }

  async function locate(id: string) {
    if (typeof id !== 'string' || !ID.test(id)) throw new StorageError('INVALID_RANGE');
    const directory = await root.getDirectoryHandle(id);
    const handle = await directory.getFileHandle('content');
    return { directory, handle, targetId: await identifyFile(handle) };
  }
  async function entry(id: string): Promise<OpfsEntry> {
    const { directory, handle, targetId } = await locate(id);
    const release = await leaseFile(targetId);
    let metadata: Metadata;
    try { metadata = await readMetadata(directory, id); }
    finally { await release(); }
    const size = BigInt(metadata.size);
    async function current() {
      const next = await readMetadata(directory, id);
      if (next.name !== metadata.name || next.size !== metadata.size) throw new StorageError('CACHE_CORRUPT');
      return next;
    }
    const source: ContentSource = Object.freeze({
      info: Object.freeze({ name: metadata.name, size }),
      async open(): Promise<SourceReader> {
        const unlock = await leaseFile(targetId);
        try {
          if ((await current()).state !== 'complete') throw new StorageError('INVALID_STATE');
          const file = await handle.getFile();
          if (BigInt(file.size) !== size) throw new StorageError('CHECKPOINT_MISMATCH');
          const reader = await fileSource(file).open();
          let closing: Promise<void> | undefined;
          return {
            read: (offset, length, settings) => reader.read(offset, length, settings),
            close() {
              closing ??= reader.close().finally(unlock);
              return closing;
            }
          };
        } catch (error) { await unlock(); throw cacheFailure(error); }
      }
    });
    const destination: ContentDestination = Object.freeze({
      targetId,
      async open(settings: Parameters<ContentDestination['open']>[0]) {
        if (settings.mode !== 'create' && settings.mode !== 'resume') throw new StorageError('INVALID_RANGE');
        validateSize(settings.size);
        if (settings.size !== size) throw new StorageError('CHECKPOINT_MISMATCH');
        if (settings.signal?.aborted) throw new StorageError('TASK_CANCELLED');
        const unlock = await leaseFile(targetId);
        try {
          const metadata = await current();
          if (metadata.state === 'complete' && settings.mode !== 'resume') throw new StorageError('INVALID_STATE');
          const file = await handle.getFile();
          if (settings.mode === 'create' && file.size > 0) throw new StorageError('DESTINATION_EXISTS');
          if (BigInt(file.size) > size) throw new StorageError('CHECKPOINT_MISMATCH');
          // Recovery is an explicit write lease: withdraw completed-source access
          // before any raw revalidation or changes, then re-finalize on success.
          if (metadata.state === 'complete') await writeMetadata(directory, { ...metadata, state: 'partial' });
          return await opfsWriter({
            path: [DIRECTORY, id, 'content'], targetId, size, maxPendingBytes, workerUrl, signal: settings.signal,
            release: unlock,
            async complete() {
              if (BigInt((await handle.getFile()).size) !== size) throw new StorageError('CHECKPOINT_MISMATCH');
              await writeMetadata(directory, { ...await current(), state: 'complete' });
            }
          });
        } catch (error) { await unlock(); throw cacheFailure(error); }
      }
    });
    return Object.freeze({
      id, targetId, name: metadata.name, size, source, destination,
      async status() {
        const unlock = await leaseFile(targetId);
        try { return (await current()).state; }
        catch (error) { throw cacheFailure(error); }
        finally { await unlock(); }
      },
      exportTo: (target: ContentDestination, settings: ExportOptions = {}) => exportEntry(source, target, settings)
    });
  }
  return Object.freeze({
    async create({ name, size }: { name: string; size: bigint }) {
      validateSize(size);
      if (typeof name !== 'string' || name.length > 1024) throw new StorageError('INVALID_RANGE');
      const id = crypto.randomUUID();
      try {
        const directory = await root.getDirectoryHandle(id, { create: true });
        await directory.getFileHandle('content', { create: true });
        await writeMetadata(directory, { version: 1, id, name, size: size.toString(), state: 'partial' });
        return await entry(id);
      } catch (error) {
        try { await root.removeEntry(id, { recursive: true }); } catch { /* preserve creation error */ }
        throw cacheFailure(error);
      }
    },
    async open(id: string) { try { return await entry(id); } catch (error) { throw cacheFailure(error); } },
    async remove(id: string) {
      try {
        const { targetId } = await locate(id);
        const unlock = await leaseFile(targetId);
        try {
          await root.removeEntry(id, { recursive: true });
          await forgetFile(targetId);
        }
        finally { await unlock(); }
      } catch (error) { throw cacheFailure(error); }
    }
  });
}

async function exportEntry(source: ContentSource, destination: ContentDestination, options: ExportOptions) {
  const chunkBytes = options.chunkBytes ?? 1024 * 1024;
  if (!Number.isSafeInteger(chunkBytes) || chunkBytes <= 0 || chunkBytes > 4 * 1024 * 1024) {
    throw new StorageError('INVALID_RANGE');
  }
  function check() { if (options.signal?.aborted) throw new StorageError('TASK_CANCELLED'); }
  const report = (copiedBytes: bigint) => {
    try { options.onProgress?.(Object.freeze({ copiedBytes, totalBytes: source.info.size })); }
    catch { /* observer errors do not change storage correctness */ }
  };
  check();
  const reader = await source.open();
  let writer: Awaited<ReturnType<ContentDestination['open']>> | undefined;
  try {
    check();
    writer = await destination.open({ mode: 'create', size: source.info.size, signal: options.signal });
    let offset = 0n;
    report(offset);
    while (offset < source.info.size) {
      check();
      const length = Number(source.info.size - offset < BigInt(chunkBytes) ? source.info.size - offset : BigInt(chunkBytes));
      const bytes = await reader.read(offset, length, { signal: options.signal });
      check();
      await writer.write(offset, bytes);
      offset += BigInt(length);
      report(offset);
    }
    check();
    const result = await writer.finish();
    check();
    return result;
  } catch (error) {
    try { await writer?.abort(); } catch { /* preserve copy failure; source stays intact */ }
    throw storageFailure(error);
  } finally { await reader.close(); }
}
