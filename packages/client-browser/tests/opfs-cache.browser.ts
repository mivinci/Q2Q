import { opfsCache, fileDestination, StorageError } from '../src/index.ts';
import type { OpfsEntry, OpfsCache, ContentDestination } from '../src/index.ts';

function assert(value: unknown, message = 'assertion failed'): asserts value {
  if (!value) throw new Error(message);
}
async function rejects(promise: Promise<unknown>, expected: string) {
  try { await promise; } catch (error) {
    assert(error instanceof StorageError && error.code === expected,
      `expected ${expected}, got ${error instanceof Error ? error.message : String(error)}`);
    return;
  }
  throw new Error(`expected ${expected}`);
}
const bytes = (value: string) => new TextEncoder().encode(value);
async function read(entry: OpfsEntry) {
  const reader = await entry.source.open();
  try { return new TextDecoder().decode(await reader.read(0n, Number(entry.size))); }
  finally { await reader.close(); }
}
async function fixture(run: (cache: OpfsCache, entry: OpfsEntry) => Promise<void>, size = 6n) {
  const cache = await opfsCache();
  const entry = await cache.create({ name: 'example.bin', size });
  try { await run(cache, entry); } finally { await cache.remove(entry.id); }
}
async function complete(entry: OpfsEntry, value = 'abcdef') {
  const writer = await entry.destination.open({ mode: 'create', size: entry.size });
  await writer.write(0n, bytes(value));
  return writer.finish();
}
async function target(run: (handle: FileSystemFileHandle, destination: ContentDestination) => Promise<void>) {
  const root = await navigator.storage.getDirectory();
  const name = `q2q-export-${crypto.randomUUID()}`;
  const handle = await root.getFileHandle(name, { create: true });
  try { await run(handle, await fileDestination(handle)); }
  finally { await root.removeEntry(name); }
}

export const cases: Record<string, () => Promise<void>> = {
  'worker writes out of order, flushes, and only finalization enables reading': async () => fixture(async (cache, entry) => {
    assert(await entry.status() === 'partial');
    await rejects(entry.source.open(), 'INVALID_STATE');
    const writer = await entry.destination.open({ mode: 'create', size: 6n });
    await writer.write(3n, bytes('def'));
    await writer.write(0n, bytes('abc'));
    await writer.commit();
    await rejects(entry.source.open(), 'DESTINATION_BUSY');
    const receipt = await writer.finish();
    assert(receipt.kind === 'staged' && receipt.needsExport && receipt.targetId === entry.targetId);
    assert(await entry.status() === 'complete');
    assert(await read(entry) === 'abcdef');
    const reopened = await cache.open(entry.id);
    assert(reopened.targetId === entry.targetId && await read(reopened) === 'abcdef');
    await rejects(reopened.destination.open({ mode: 'create', size: 6n }), 'INVALID_STATE');
  }),
  'abort preserves raw bytes for revalidation but does not enable completed reads': async () => fixture(async (cache, entry) => {
    const writer = await entry.destination.open({ mode: 'create', size: 6n });
    await writer.write(0n, bytes('abc'));
    await writer.commit();
    await writer.abort();
    assert(await entry.status() === 'partial');
    await rejects(entry.source.open(), 'INVALID_STATE');
    const reopened = await cache.open(entry.id);
    const resumed = await reopened.destination.open({ mode: 'resume', size: 6n });
    await resumed.write(3n, bytes('def'));
    await resumed.finish();
    assert(await read(reopened) === 'abcdef');
  }),
  'incomplete file cannot be finalized and its cache remains partial': async () => fixture(async (_cache, entry) => {
    const writer = await entry.destination.open({ mode: 'create', size: 6n });
    await writer.write(0n, bytes('abc'));
    await rejects(writer.finish(), 'CHECKPOINT_MISMATCH');
    assert(await entry.status() === 'partial');
    await rejects(entry.source.open(), 'INVALID_STATE');
    const resumed = await entry.destination.open({ mode: 'resume', size: 6n });
    await resumed.write(3n, bytes('def'));
    await resumed.finish();
  }),
  'empty cache finalizes and exports without a whole-file allocation': async () => fixture(async (_cache, entry) => {
    const writer = await entry.destination.open({ mode: 'create', size: 0n });
    await writer.finish();
    await target(async (handle, destination) => {
      await entry.exportTo(destination);
      assert((await handle.getFile()).size === 0);
    });
  }, 0n),
  'export is chunked, observable, and retains source on success': async () => fixture(async (_cache, entry) => {
    await complete(entry);
    await target(async (handle, destination) => {
      const progress: bigint[] = [];
      await entry.exportTo(destination, { chunkBytes: 2, onProgress: value => progress.push(value.copiedBytes) });
      assert(progress.join(',') === '0,2,4,6');
      assert(await (await handle.getFile()).text() === 'abcdef');
      assert(await read(entry) === 'abcdef');
    });
  }),
  'failed export leaves source intact and can retry to the same target': async () => fixture(async (_cache, entry) => {
    await complete(entry);
    await target(async (handle, destination) => {
      const original = handle.createWritable.bind(handle);
      handle.createWritable = async settings => {
        const stream = await original(settings);
        stream.write = async () => { throw new DOMException('private path', 'QuotaExceededError'); };
        return stream;
      };
      await rejects(entry.exportTo(destination, { chunkBytes: 2 }), 'STORAGE_FULL');
      assert(await read(entry) === 'abcdef');
      handle.createWritable = original;
      await entry.exportTo(destination, { chunkBytes: 2 });
      assert(await (await handle.getFile()).text() === 'abcdef');
    });
  }),
  'cancelled export aborts target and keeps original cache for retry': async () => fixture(async (_cache, entry) => {
    await complete(entry);
    await target(async (handle, destination) => {
      const controller = new AbortController();
      await rejects(entry.exportTo(destination, {
        chunkBytes: 2, signal: controller.signal,
        onProgress: value => { if (value.copiedBytes === 2n) controller.abort(); }
      }), 'TASK_CANCELLED');
      assert((await handle.getFile()).size === 0);
      assert(await read(entry) === 'abcdef');
      await entry.exportTo(destination);
    });
  }),
  'cleanup refuses active writers, readers, and exports': async () => fixture(async (cache, entry) => {
    const writer = await entry.destination.open({ mode: 'create', size: 6n });
    await rejects(cache.remove(entry.id), 'DESTINATION_BUSY');
    await writer.write(0n, bytes('abcdef'));
    await writer.finish();
    const reader = await entry.source.open();
    await rejects(cache.remove(entry.id), 'DESTINATION_BUSY');
    await reader.close();
    await target(async (_handle, destination) => {
      let attempted = false;
      let deletion: Promise<void> | undefined;
      const delayed: ContentDestination = {
        targetId: destination.targetId,
        async open(options) {
          const writer = await destination.open(options);
          return { ...writer, async write(offset, bytes) {
            // Keep export active until the asynchronous removal attempt arrives
            // at the target lock, rather than racing the end of a tiny copy.
            await deletion;
            await writer.write(offset, bytes);
          }};
        }
      };
      await entry.exportTo(delayed, { chunkBytes: 2, onProgress() {
        if (!attempted) { attempted = true; deletion = rejects(cache.remove(entry.id), 'DESTINATION_BUSY'); }
      }});
      await deletion;
    });
  }),
  'invalid cache identifiers, sizes, and export chunk limits are rejected': async () => fixture(async (cache, entry) => {
    await rejects(cache.open('../outside'), 'INVALID_RANGE');
    await rejects(cache.remove(''), 'INVALID_RANGE');
    await rejects(cache.create({ name: 'x', size: -1n }), 'INVALID_RANGE');
    await rejects(cache.create({ name: 'x', size: 2n ** 60n }), 'UNSUPPORTED_CAPABILITY');
    await complete(entry);
    await target(async (_handle, destination) => {
      await rejects(entry.exportTo(destination, { chunkBytes: 0 }), 'INVALID_RANGE');
      await rejects(entry.exportTo(destination, { chunkBytes: 4 * 1024 * 1024 + 1 }), 'INVALID_RANGE');
      await rejects(entry.exportTo(destination, { signal: AbortSignal.abort() }), 'TASK_CANCELLED');
    });
  }),
  'queued cancellation fences worker writes and releases the lease': async () => fixture(async (_cache, entry) => {
    const writer = await entry.destination.open({ mode: 'create', size: 6n });
    const writing = rejects(writer.write(0n, bytes('abcdef')), 'TASK_CANCELLED');
    const aborting = writer.abort();
    assert(writer.abort() === aborting);
    await Promise.all([writing, aborting]);
    const next = await entry.destination.open({ mode: 'resume', size: 6n });
    await next.abort();
  }),
  'pending worker bytes are bounded and caller buffers are copied': async () => {
    const cache = await opfsCache({ maxPendingBytes: 6 });
    const entry = await cache.create({ name: 'bounded', size: 6n });
    try {
      const writer = await entry.destination.open({ mode: 'create', size: 6n });
      const input = bytes('abcdef');
      const pending = writer.write(0n, input);
      input.fill(0);
      await rejects(writer.write(0n, bytes('a')), 'RESOURCE_LIMIT');
      await pending;
      await writer.finish();
      assert(await read(entry) === 'abcdef');
    } finally { await cache.remove(entry.id); }
  },
  'worker handles short native writes without dropping bytes': async () => {
    const cache = await opfsCache({ workerUrl: new URL('/tests/opfs-fault.worker.ts?mode=partial', location.href) });
    const entry = await cache.create({ name: 'partial writes', size: 6n });
    try { await complete(entry); assert(await read(entry) === 'abcdef'); }
    finally { await cache.remove(entry.id); }
  },
  'worker write, flush, close and zero-progress failures release all resources': async () => {
    for (const [mode, code] of [['write', 'STORAGE_FULL'], ['flush', 'STORAGE_FULL'],
      ['close', 'PERMISSION_REQUIRED'], ['zero', 'STORAGE_UNAVAILABLE']]) {
      const cache = await opfsCache({ workerUrl: new URL(`/tests/opfs-fault.worker.ts?mode=${mode}`, location.href) });
      const entry = await cache.create({ name: 'fault', size: 6n });
      try {
        const writer = await entry.destination.open({ mode: 'create', size: 6n });
        if (mode === 'write' || mode === 'zero') {
          await rejects(writer.write(0n, bytes('abcdef')), code!);
        } else {
          await writer.write(0n, bytes('abcdef'));
          await rejects(writer.finish(), code!);
        }
        // In particular, abort after a failed close must not send RPC to a dead worker.
        await writer.abort();
        assert(await entry.status() === 'partial');
        const healthy = await (await opfsCache()).open(entry.id);
        const next = await healthy.destination.open({ mode: 'resume', size: 6n });
        await next.abort();
      } finally { await cache.remove(entry.id); }
    }
  },
  'idle worker crashes release the target without requiring a caller command': async () => {
    const cache = await opfsCache({ workerUrl: new URL('/tests/opfs-fault.worker.ts?mode=crash', location.href) });
    const entry = await cache.create({ name: 'crash', size: 6n });
    try {
      const writer = await entry.destination.open({ mode: 'create', size: 6n });
      await new Promise(resolve => setTimeout(resolve, 150));
      assert(await entry.status() === 'partial');
      await rejects(writer.write(0n, bytes('abcdef')), 'STORAGE_UNAVAILABLE');
      await writer.abort();
      const healthy = await (await opfsCache()).open(entry.id);
      const next = await healthy.destination.open({ mode: 'resume', size: 6n });
      await next.abort();
    } finally { await cache.remove(entry.id); }
  },
  'broken worker URL fails safely and allows reopening with the default worker': async () => {
    const bad = await opfsCache({ workerUrl: new URL('/not-a-worker.ts', location.href) });
    const entry = await bad.create({ name: 'broken', size: 1n });
    try {
      await rejects(entry.destination.open({ mode: 'create', size: 1n }), 'STORAGE_UNAVAILABLE');
      const good = await (await opfsCache()).open(entry.id);
      const writer = await good.destination.open({ mode: 'create', size: 1n });
      await writer.abort();
    } finally { await bad.remove(entry.id); }
  }
};

let held: Awaited<ReturnType<ContentDestination['open']>> | undefined;
export async function createForReload() {
  const cache = await opfsCache();
  const entry = await cache.create({ name: 'reload.bin', size: 6n });
  await complete(entry);
  return { id: entry.id, targetId: entry.targetId };
}
export async function checkAfterReload(id: string) {
  const entry = await (await opfsCache()).open(id);
  assert(await read(entry) === 'abcdef');
  return entry.targetId;
}
export async function holdPartial() {
  const cache = await opfsCache();
  const entry = await cache.create({ name: 'interrupted.bin', size: 6n });
  held = await entry.destination.open({ mode: 'create', size: 6n });
  await held.write(0n, bytes('abc'));
  await held.commit();
  return entry.id;
}
export async function resumeAfterReload(id: string) {
  const entry = await (await opfsCache()).open(id);
  assert(await entry.status() === 'partial');
  const writer = await entry.destination.open({ mode: 'resume', size: 6n });
  await writer.write(3n, bytes('def'));
  await writer.finish();
  assert(await read(entry) === 'abcdef');
}
export async function remove(id: string) {
  const cache = await opfsCache();
  await cache.remove(id);
  await rejects(cache.open(id), 'CACHE_NOT_FOUND');
}
