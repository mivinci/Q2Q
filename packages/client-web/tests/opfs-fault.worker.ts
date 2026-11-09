import type { SyncFileHandle } from '../src/opfs-protocol.ts';

// Faults wrap a real sync handle, so the tests exercise RPC and native cleanup.
const mode = new URL(location.href).searchParams.get('mode');
const prototype = FileSystemFileHandle.prototype as SyncFileHandle;
const open = prototype.createSyncAccessHandle;
prototype.createSyncAccessHandle = async function () {
  const file = await open.call(this);
  const write = file.write.bind(file);
  const close = file.close.bind(file);
  if (mode === 'crash') {
    setTimeout(() => { throw new Error('private worker details'); }, 50);
  } else if (mode === 'partial') {
    file.write = (bytes, options) => write(bytes.subarray(0, Math.min(2, bytes.length)), options);
  } else if (mode === 'flush') {
    file.flush = () => { throw new DOMException('private/path', 'QuotaExceededError'); };
  } else if (mode === 'write') {
    file.write = () => { throw new DOMException('private/path', 'QuotaExceededError'); };
  } else if (mode === 'close') {
    file.close = () => { close(); throw new DOMException('private/path', 'NotAllowedError'); };
  } else if (mode === 'zero') {
    file.write = () => 0;
  }
  return file;
};

// Exercise a bundler wrapper which installs the RPC handler asynchronously.
await new Promise(resolve => setTimeout(resolve, 25));
await import('../src/opfs-worker.ts');
