import { StorageError, storageFailure } from './error.ts';

/** Content-addressed metadata cache: one minimal seed per info hash. Records
 * hold no display names or Track URLs; both are re-derived from verified bytes. */
const DATABASE = 'q2q-magnet-v1';
const LIMIT = 256;

export interface MagnetCache {
  get(hash: string): Promise<Uint8Array | null>;
  put(hash: string, bytes: Uint8Array): Promise<void>;
  close(): Promise<void>;
}

let opened: Promise<MagnetCache | null> | undefined;

/** A shared instance is safe: keys are content identities and writes are idempotent.
 * Returns a no-op cache when IndexedDB is unavailable; caching is an optimization. */
export function magnetCache(): Promise<MagnetCache | null> {
  opened ??= (async () => {
    if (!globalThis.indexedDB) return null;
    const db = await new Promise<IDBDatabase>((resolve, reject) => {
      const request = indexedDB.open(DATABASE, 1);
      request.onupgradeneeded = () => {
        if (!request.result.objectStoreNames.contains('magnets')) {
          const store = request.result.createObjectStore('magnets', { keyPath: 'hash' });
          store.createIndex('writtenAt', 'writtenAt');
        }
      };
      request.onerror = () => reject(storageFailure(request.error));
      request.onsuccess = () => resolve(request.result);
    });
    let closed = false;
    const pending = new Set<Promise<unknown>>();
    function track<T>(operation: () => Promise<T>): Promise<T> {
      if (closed) return Promise.reject(new StorageError('STATE_CLOSED'));
      const result = operation().catch(error => { throw storageFailure(error); });
      pending.add(result);
      void result.then(() => pending.delete(result), () => pending.delete(result));
      return result;
    }
    function range(hash: unknown): asserts hash is string {
      if (typeof hash !== 'string' || !/^sha256:[a-f0-9]{64}$/.test(hash)) throw new StorageError('INVALID_RANGE');
    }
    return {
      get(hash: string) {
        try { range(hash); } catch (error) { return Promise.reject(error); }
        return track(() => new Promise<Uint8Array | null>((resolve, reject) => {
          const tx = db.transaction('magnets', 'readonly');
          const request = tx.objectStore('magnets').get(hash);
          request.onsuccess = () => {
            const record = request.result as { hash: string; bytes: Uint8Array } | undefined;
            resolve(record && record.bytes instanceof Uint8Array && record.bytes.length > 0 && record.bytes.length <= 16 * 1024
              ? new Uint8Array(record.bytes)
              : null);
          };
          request.onerror = () => reject(request.error);
        }));
      },
      put(hash: string, bytes: Uint8Array) {
        try {
          range(hash);
          if (!(bytes instanceof Uint8Array) || bytes.length === 0 || bytes.length > 16 * 1024) throw new StorageError('INVALID_RANGE');
        } catch (error) { return Promise.reject(error); }
        return track(() => new Promise<void>((resolve, reject) => {
          const tx = db.transaction('magnets', 'readwrite');
          const store = tx.objectStore('magnets');
          // Refresh recency; a bounded store never grows with link reuse.
          store.put({ hash, bytes: new Uint8Array(bytes), writtenAt: Date.now() });
          const count = store.count();
          count.onsuccess = () => {
            let excess = count.result - LIMIT;
            if (excess <= 0) return;
            const cursor = store.index('writtenAt').openCursor();
            cursor.onsuccess = () => {
              const value = cursor.result;
              if (!value || excess-- <= 0) return;
              value.delete();
              value.continue();
            };
          };
          tx.oncomplete = () => resolve();
          tx.onabort = () => reject(tx.error);
          tx.onerror = () => reject(tx.error);
        }));
      },
      close() {
        if (closed) return Promise.resolve();
        closed = true;
        return (async () => { await Promise.allSettled([...pending]); db.close(); })();
      }
    };
  })().catch(() => null);
  return opened;
}
