import { StorageError } from './error.ts';
import type { ContentSource, SourceReader } from './storage.ts';

const DEFAULT_MAX_READ_BYTES = 4 * 1024 * 1024;

/** Adapts an already-selected File snapshot; never opens a picker or reads eagerly. */
export function fileSource(
  file: File,
  options: { maxReadBytes?: number } = {}
): ContentSource {
  const maxReadBytes = options.maxReadBytes ?? DEFAULT_MAX_READ_BYTES;
  if (!Number.isSafeInteger(maxReadBytes) || maxReadBytes <= 0) {
    throw new StorageError('INVALID_RANGE');
  }
  if (!Number.isSafeInteger(file.size) || file.size < 0) {
    throw new StorageError('UNSUPPORTED_CAPABILITY');
  }
  // File.size is a host Number. Preserve its checked value before exposing bigint.
  const size = BigInt(file.size);
  const info = Object.freeze({ name: file.name, size });
  return Object.freeze({
    info,
    async open(): Promise<SourceReader> {
      let closed = false;
      let closing: Promise<void> | undefined;
      const active = new Set<Promise<Uint8Array>>();

      async function readRange(
        offset: bigint,
        length: number,
        signal?: AbortSignal
      ): Promise<Uint8Array> {
        try {
          // File reads cannot reliably interrupt host IO. Cancellation suppresses
          // delivery, while the lease remains live until that IO has settled.
          const bytes = await file.slice(Number(offset), Number(offset) + length).arrayBuffer();
          if (signal?.aborted || closed) throw new StorageError('TASK_CANCELLED');
          if (bytes.byteLength !== length) throw new StorageError('SOURCE_CHANGED');
          return new Uint8Array(bytes);
        } catch (error) {
          if (signal?.aborted || closed) throw new StorageError('TASK_CANCELLED');
          if (error instanceof StorageError) throw error;
          if (error instanceof Error && error.name === 'NotAllowedError') {
            throw new StorageError('PERMISSION_REQUIRED');
          }
          throw new StorageError('SOURCE_UNAVAILABLE');
        }
      }

      return {
        read(offset, length, readOptions = {}) {
          if (closed) return Promise.reject(new StorageError('INVALID_STATE'));
          if (readOptions.signal?.aborted) return Promise.reject(new StorageError('TASK_CANCELLED'));
          if (typeof offset !== 'bigint' || offset < 0n || !Number.isSafeInteger(length)
            || length < 0 || length > maxReadBytes || offset > size
            || BigInt(length) > size - offset) {
            return Promise.reject(new StorageError('INVALID_RANGE'));
          }
          const pending = readRange(offset, length, readOptions.signal);
          active.add(pending);
          // Both handlers resolve: cleanup never creates an unobserved rejection.
          void pending.then(() => active.delete(pending), () => active.delete(pending));
          return pending;
        },
        close() {
          if (!closing) {
            closed = true;
            closing = Promise.allSettled([...active]).then(() => undefined);
          }
          return closing;
        }
      };
    }
  });
}
