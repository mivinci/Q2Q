import { StorageError, storageFailure } from './error.ts';
import type { OpfsRequest, OpfsResponse, SyncFile, SyncFileHandle } from './opfs-protocol.ts';

const scope = globalThis as unknown as {
  onmessage: ((event: MessageEvent<OpfsRequest>) => void) | null;
  postMessage(message: OpfsResponse, transfer?: Transferable[]): void;
  close(): void;
};
let file: SyncFile | undefined;
let tail: Promise<void> = Promise.resolve();
scope.onmessage = ({ data }) => {
  // Initialization is asynchronous; do not let another command overtake it.
  tail = tail.then(async () => {
    try {
      if (data.type === 'open') {
        if (file) throw new StorageError('INVALID_STATE');
        // Resolve app-owned OPFS paths inside the Worker. Some engines support
        // sync access but cannot structured-clone FileSystemHandle objects.
        if (!Array.isArray(data.path) || data.path.length < 1 || data.path.length > 8
          || data.path.some(part => typeof part !== 'string' || !part || part === '.' || part === '..'
            || part.includes('/') || part.length > 255)) throw new StorageError('INVALID_RANGE');
        let directory = await navigator.storage.getDirectory();
        for (const part of data.path.slice(0, -1)) directory = await directory.getDirectoryHandle(part);
        const handle = await directory.getFileHandle(data.path[data.path.length - 1]!) as SyncFileHandle;
        if (!handle.createSyncAccessHandle) throw new StorageError('UNSUPPORTED_CAPABILITY');
        file = await handle.createSyncAccessHandle();
      } else {
        if (!file) throw new StorageError('INVALID_STATE');
        if (data.type === 'write') {
          let written = 0;
          while (written < data.bytes.length) {
            const count = file.write(data.bytes.subarray(written), { at: data.offset + written });
            if (!Number.isSafeInteger(count) || count <= 0 || count > data.bytes.length - written) {
              throw new StorageError('STORAGE_UNAVAILABLE');
            }
            written += count;
          }
        } else if (data.type === 'read') {
          let bytes: Uint8Array<ArrayBuffer> | null = null;
          if (data.offset + data.length <= file.getSize()) {
            bytes = new Uint8Array(data.length);
            let received = 0;
            while (received < bytes.length) {
              const count = file.read(bytes.subarray(received), { at: data.offset + received });
              if (!Number.isSafeInteger(count) || count <= 0 || count > bytes.length - received) {
                throw new StorageError('SOURCE_UNAVAILABLE');
              }
              received += count;
            }
          }
          scope.postMessage({ id: data.id, bytes }, bytes ? [bytes.buffer] : []);
          return;
        } else if (data.type === 'flush') {
          file.flush();
        } else {
          file.close();
          file = undefined;
        }
      }
      scope.postMessage({ id: data.id });
      if (data.type === 'close') scope.close();
    } catch (error) {
      // The host owns cleanup after a failed operation; no exception text crosses.
      scope.postMessage({ id: data.id, error: storageFailure(error).code });
    }
  });
};

// Handshake prevents messages being lost while a module wrapper initializes.
scope.postMessage({ id: 0 });
