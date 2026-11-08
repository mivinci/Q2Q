import { StorageError, storageFailure } from './error.ts';

/** Identity registry is origin-wide so independently bundled SDK copies cooperate. */
const DATABASE = 'q2q-file-targets-v1';
const REGISTRY_LOCK = 'q2q:file-targets:v1';

interface TargetRecord { id: string; handle: FileSystemFileHandle }

function database(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(DATABASE, 1);
    request.onupgradeneeded = () => request.result.createObjectStore('targets', { keyPath: 'id' });
    request.onerror = () => reject(request.error);
    request.onsuccess = () => {
      const db = request.result;
      db.onversionchange = () => db.close();
      resolve(db);
    };
  });
}

/** No transaction stays open while awaiting asynchronous filesystem comparisons. */
export async function identifyFile(handle: FileSystemFileHandle): Promise<string> {
  if (!globalThis.navigator?.locks || !navigator.storage?.getDirectory) {
    throw new StorageError('UNSUPPORTED_CAPABILITY');
  }
  try {
    const root = await navigator.storage.getDirectory();
    const path = await root.resolve(handle);
    if (path !== null) {
      // OPFS paths identify logical app-owned targets. Managed cache directories
      // contain random IDs and are never reused. Safari cannot clone handles
      // into IDB, so OPFS must not depend on the external-file handle registry.
      const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(JSON.stringify(path)));
      return 'opfs-v1:' + Array.from(new Uint8Array(digest), value => value.toString(16).padStart(2, '0')).join('');
    }
    if (!globalThis.indexedDB) throw new StorageError('UNSUPPORTED_CAPABILITY');
    return await navigator.locks.request(REGISTRY_LOCK, async () => {
      const db = await database();
      try {
        // Compare bounded batches: old target history must not load every handle
        // into memory at once. The registry lock prevents concurrent SDK inserts.
        let after: string | undefined;
        for (;;) {
          const records = await new Promise<TargetRecord[]>((resolve, reject) => {
            const transaction = db.transaction('targets', 'readonly');
            const range = after === undefined ? undefined : IDBKeyRange.lowerBound(after, true);
            const request = transaction.objectStore('targets').getAll(range, 128);
            transaction.oncomplete = () => resolve(request.result);
            transaction.onabort = () => reject(transaction.error);
            transaction.onerror = () => {}; // transaction abort owns rejection
          });
          for (const record of records) {
            if (await handle.isSameEntry(record.handle)) return record.id;
          }
          if (records.length < 128) break;
          after = records[records.length - 1]!.id;
        }
        const id = crypto.randomUUID();
        await new Promise<void>((resolve, reject) => {
          const transaction = db.transaction('targets', 'readwrite');
          transaction.objectStore('targets').add({ id, handle } satisfies TargetRecord);
          transaction.oncomplete = () => resolve();
          transaction.onabort = () => reject(transaction.error);
          transaction.onerror = () => {};
        });
        return id;
      } finally {
        db.close();
      }
    });
  } catch (error) { throw storageFailure(error); }
}

/** Holds the Web Lock across stream close/reopen; contention fails immediately. */
export async function leaseFile(targetId: string): Promise<() => Promise<void>> {
  let release!: () => void;
  const held = new Promise<void>(resolve => { release = resolve; });
  let acquired!: () => void;
  let reject!: (error: unknown) => void;
  const ready = new Promise<void>((resolve, fail) => { acquired = resolve; reject = fail; });
  const request = navigator.locks.request(`q2q:file:${targetId}`, { ifAvailable: true }, async lock => {
    if (!lock) throw new StorageError('DESTINATION_BUSY');
    acquired();
    await held;
  });
  void request.catch(reject);
  await ready;
  return async () => { release(); await request; };
}

/** Removes only a deleted cache's identity record; never deletes the file itself. */
export async function forgetFile(targetId: string): Promise<void> {
  if (targetId.startsWith('opfs-v1:')) return;
  try {
    await navigator.locks.request(REGISTRY_LOCK, async () => {
      const db = await database();
      try {
        await new Promise<void>((resolve, reject) => {
          const transaction = db.transaction('targets', 'readwrite');
          transaction.objectStore('targets').delete(targetId);
          transaction.oncomplete = () => resolve();
          transaction.onabort = () => reject(transaction.error);
          transaction.onerror = () => {};
        });
      } finally { db.close(); }
    });
  } catch (error) { throw storageFailure(error); }
}
