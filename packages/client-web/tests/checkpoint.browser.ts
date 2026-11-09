import init, { SeedBuilder } from '@q2q/wasm';
import { browserState, opfsCache, createBlockVerifier, resumeCheckpoint, fileDestination, StorageError } from '../src/index.ts';
import type { StateStore, ContentDestination, CheckpointSession } from '../src/index.ts';

const BLOCK = 262144;
function assert(value: unknown, message = 'assertion failed'): asserts value { if (!value) throw new Error(message); }
async function rejects(promise: Promise<unknown>, expected: string) {
  try { await promise; } catch (error) {
    assert(error instanceof StorageError && error.code === expected,
      `expected ${expected}, got ${error instanceof Error ? error.message : String(error)}`);
    return;
  }
  throw new Error(`expected ${expected}`);
}
async function content(name = 'recovery.bin', size = 2 * BLOCK + 17) {
  await init();
  const bytes = new Uint8Array(size);
  for (let i = 0; i < bytes.length; i++) bytes[i] = (i * 31 + Math.floor(i / BLOCK)) % 251;
  const builder = new SeedBuilder(bytes.length);
  builder.update(bytes);
  const prepared = builder.finish(name);
  const seed = prepared.seed_bytes();
  const proof = prepared.proof_bytes();
  prepared.free();
  const verifier = await createBlockVerifier({ seed, proof });
  return { bytes, seed, proof, verifier };
}
async function fixture(run: (value: Awaited<ReturnType<typeof makeFixture>>) => Promise<void>, size?: number) {
  const value = await makeFixture(size);
  try { await run(value); } finally {
    for (const session of value.sessions) { try { await session.abort(); } catch {} }
    try { await value.state.discard(value.checkpoint.id); } catch {}
    await value.state.close();
    await value.cache.remove(value.entry.id);
    value.verifier.close();
  }
}
async function makeFixture(size?: number) {
  const material = await content('recovery.bin', size);
  const namespace = `test-${crypto.randomUUID()}`;
  const state = await browserState({ namespace });
  const cache = await opfsCache();
  const entry = await cache.create({ name: 'recovery.bin', size: material.verifier.content.size });
  const checkpoint = await state.create({ content: material.verifier.content, targetId: entry.targetId });
  const sessions: CheckpointSession[] = [];
  return { ...material, namespace, state, cache, entry, checkpoint, sessions,
    async resume(settings: { state?: StateStore; destination?: ContentDestination; signal?: AbortSignal } = {}) {
      const session = await resumeCheckpoint({ state: settings.state ?? state, checkpointId: checkpoint.id,
        verifier: material.verifier, destination: settings.destination ?? entry.destination, signal: settings.signal });
      sessions.push(session);
      return session;
    },
    block(index: number) { return material.bytes.subarray(index * BLOCK, (index + 1) * BLOCK); }
  };
}
async function mutate(namespace: string, id: string, run: (value: any) => void) {
  const db = await new Promise<IDBDatabase>((resolve, reject) => {
    const request = indexedDB.open('q2q-checkpoints-v1');
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
  try {
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction('checkpoints', 'readwrite');
      const store = tx.objectStore('checkpoints');
      const request = store.get([namespace, id]);
      request.onsuccess = () => { run(request.result); store.put(request.result); };
      tx.oncomplete = () => resolve(); tx.onabort = () => reject(tx.error);
    });
  } finally { db.close(); }
}

export const cases: Record<string, () => Promise<void>> = {
  'checkpoint CAS is atomic, copied, and rejects stale revisions': async () => fixture(async f => {
    const lease = await f.state.lease(f.checkpoint.id);
    try {
      const bitmap = new Uint8Array([1]);
      const update = lease.replace(1, bitmap);
      bitmap[0] = 7;
      const next = await update;
      assert(next.revision === 2 && next.committedBytes === BigInt(BLOCK));
      next.committed[0] = 0;
      await rejects(lease.replace(1, new Uint8Array([7])), 'CHECKPOINT_CONFLICT');
      assert((await f.state.read(next.id)).committed[0] === 1);
    } finally { await lease.close(); }
  }),
  'namespace isolation and bounded pagination preserve all records': async () => fixture(async f => {
    for (let i = 0; i < 4; i++) await f.state.create({ content: f.verifier.content, targetId: f.entry.targetId });
    const ids = new Set<string>();
    let after: string | undefined;
    do {
      const page = await f.state.list({ limit: 2, after });
      for (const item of page.items) ids.add(item.id);
      after = page.next ?? undefined;
    } while (after);
    assert(ids.size === 5);
    const other = await browserState({ namespace: `other-${crypto.randomUUID()}` });
    try {
      assert((await other.list()).items.length === 0);
      await rejects(other.read(f.checkpoint.id), 'CHECKPOINT_NOT_FOUND');
    } finally { await other.close(); }
  }),
  'active checkpoint leases block deletion and other stores': async () => fixture(async f => {
    const session = await f.resume();
    await rejects(f.state.discard(f.checkpoint.id), 'CHECKPOINT_BUSY');
    const other = await browserState({ namespace: f.namespace });
    try { await rejects(other.lease(f.checkpoint.id), 'CHECKPOINT_BUSY'); }
    finally { await other.close(); }
    await session.abort();
    const lease = await f.state.lease(f.checkpoint.id);
    await lease.close();
  }),
  'wrong seed or target is rejected before opening target IO': async () => fixture(async f => {
    const another = await content('different identity');
    let opened = false;
    const target: ContentDestination = { targetId: f.entry.targetId, async open(options) { opened = true; return f.entry.destination.open(options); } };
    try {
      await rejects(resumeCheckpoint({ state: f.state, checkpointId: f.checkpoint.id,
        verifier: another.verifier, destination: target }), 'CHECKPOINT_MISMATCH');
      await rejects(f.resume({ destination: { ...target, targetId: crypto.randomUUID() } }), 'CHECKPOINT_MISMATCH');
      assert(!opened);
    } finally { another.verifier.close(); }
  }),
  'invalid seed or unpinned proof cannot create a block verifier': async () => fixture(async f => {
    await rejects(createBlockVerifier({ seed: new Uint8Array([1, 2, 3]) }), 'INVALID_SEED');
    const proof = new Uint8Array(f.proof); proof[0]! ^= 1;
    await rejects(createBlockVerifier({ seed: f.seed, proof }), 'INVALID_SEED');
    assert(f.verifier.verify(0n, f.block(0)));
    assert(!f.verifier.verify(0n, f.block(0).subarray(1)));
  }),
  'verified blocks are not checkpointed until target commit': async () => fixture(async f => {
    const session = await f.resume();
    await session.writeBlock(1n, f.block(1));
    await session.writeBlock(0n, f.block(0));
    assert((await f.state.read(f.checkpoint.id)).committedBytes === 0n);
    const next = await session.commit();
    assert(next.committedBytes === 2n * BigInt(BLOCK));
    await session.writeBlock(2n, f.block(2));
    const result = await session.finish();
    assert(result.kind === 'staged');
    assert((await f.state.read(f.checkpoint.id)).committedBytes === BigInt(f.bytes.length));
    const reader = await f.entry.source.open();
    try {
      const actual = await reader.read(0n, f.bytes.length);
      assert(actual.every((value, i) => value === f.bytes[i]));
    } finally { await reader.close(); }
  }),
  'checkpoint recovery also works through the file-stream adapter without trusting unindexed bytes': async () => fixture(async f => {
    const root = await navigator.storage.getDirectory();
    const name = `q2q-checkpoint-file-${crypto.randomUUID()}`;
    const handle = await root.getFileHandle(name, { create: true });
    const destination = await fileDestination(handle);
    const checkpoint = await f.state.create({ content: f.verifier.content, targetId: destination.targetId });
    let session: CheckpointSession | undefined;
    try {
      session = await resumeCheckpoint({ state: f.state, checkpointId: checkpoint.id, verifier: f.verifier, destination });
      await session.writeBlock(0n, f.block(0)); await session.commit(); await session.abort();
      const stream = await handle.createWritable({ keepExistingData: true });
      await stream.write({ type: 'write', position: BLOCK, data: new Uint8Array(f.block(1)) });
      await stream.close();
      session = await resumeCheckpoint({ state: f.state, checkpointId: checkpoint.id, verifier: f.verifier, destination });
      assert(session.snapshot().committedBytes === BigInt(BLOCK));
      await session.writeBlock(1n, f.block(1)); await session.writeBlock(2n, f.block(2));
      await session.finish();
      const actual = new Uint8Array(await (await handle.getFile()).arrayBuffer());
      assert(actual.every((value, index) => value === f.bytes[index]));
    } finally {
      await session?.abort();
      await f.state.discard(checkpoint.id);
      await root.removeEntry(name);
    }
  }),
  'bad input never becomes a stored trusted block and the session stays usable': async () => fixture(async f => {
    const session = await f.resume();
    const corrupt = new Uint8Array(f.block(0)); corrupt[0]! ^= 1;
    await rejects(session.writeBlock(0n, corrupt), 'CONTENT_MISMATCH');
    await rejects(session.writeBlock(10n, f.block(0)), 'INVALID_RANGE');
    await session.writeBlock(0n, f.block(0));
    assert((await session.commit()).committedBytes === BigInt(BLOCK));
  }),
  'recovery repairs corrupt committed bytes instead of trusting the bitmap': async () => fixture(async f => {
    const first = await f.resume();
    await first.writeBlock(0n, f.block(0));
    await first.writeBlock(1n, f.block(1));
    await first.commit();
    await first.abort();
    const raw = await f.entry.destination.open({ mode: 'resume', size: f.entry.size });
    const corrupt = new Uint8Array(f.block(1)); corrupt[0]! ^= 1;
    await raw.write(BigInt(BLOCK), corrupt);
    await raw.commit(); await raw.abort();
    const next = await f.resume();
    assert(next.snapshot().committed[0] === 1);
    assert((await f.state.read(f.checkpoint.id)).committedBytes === BigInt(BLOCK));
  }),
  'truncated targets retain only complete blocks that still verify': async () => fixture(async f => {
    const first = await f.resume();
    await first.writeBlock(0n, f.block(0)); await first.writeBlock(1n, f.block(1));
    await first.commit(); await first.abort();
    const root = await navigator.storage.getDirectory();
    const directory = await (await root.getDirectoryHandle('q2q-cache-v1')).getDirectoryHandle(f.entry.id);
    const stream = await (await directory.getFileHandle('content')).createWritable({ keepExistingData: true });
    await stream.truncate(BLOCK + 3); await stream.close();
    const next = await f.resume();
    assert(next.snapshot().committedBytes === BigInt(BLOCK));
    assert(next.snapshot().committed[0] === 1);
  }),
  'empty files can checkpoint, finish, and recover without an invented block': async () => fixture(async f => {
    const first = await f.resume();
    assert(first.snapshot().committed.length === 0);
    await first.finish();
    const next = await f.resume();
    assert(next.snapshot().committedBytes === 0n);
    await next.finish();
    assert(await f.entry.status() === 'complete');
  }, 0),
  'checkpoint ahead of missing target bytes is repaired to zero': async () => fixture(async f => {
    const lease = await f.state.lease(f.checkpoint.id);
    await lease.replace(1, new Uint8Array([7])); await lease.close();
    const session = await f.resume();
    assert(session.snapshot().committedBytes === 0n);
    assert((await f.state.read(f.checkpoint.id)).committed[0] === 0);
  }),
  'flush before failed metadata update leaves no falsely committed progress': async () => fixture(async f => {
    let fail = true;
    const failing: StateStore = {
      ...f.state,
      async lease(id) {
        const lease = await f.state.lease(id);
        return { ...lease, get closed() { return lease.closed; }, replace(revision, bitmap) {
          if (fail && bitmap.some(value => value !== 0)) {
            fail = false;
            return Promise.reject(new StorageError('STORAGE_FULL'));
          }
          return lease.replace(revision, bitmap);
        }};
      }
    };
    const first = await f.resume({ state: failing });
    await first.writeBlock(0n, f.block(0));
    await rejects(first.commit(), 'STORAGE_FULL');
    assert((await f.state.read(f.checkpoint.id)).committedBytes === 0n);
    const next = await f.resume();
    assert(next.snapshot().committedBytes === 0n);
    await next.writeBlock(0n, f.block(0));
    await next.commit();
  }),
  'IDB transaction abort preserves the preceding revision and bitmap': async () => fixture(async f => {
    const lease = await f.state.lease(f.checkpoint.id);
    const put = IDBObjectStore.prototype.put;
    IDBObjectStore.prototype.put = function (...args) {
      const request = put.apply(this, args);
      if (this.transaction.db.name === 'q2q-checkpoints-v1') this.transaction.abort();
      return request;
    };
    try { await rejects(lease.replace(1, new Uint8Array([1])), 'STORAGE_UNAVAILABLE'); }
    finally { IDBObjectStore.prototype.put = put; }
    const record = await f.state.read(f.checkpoint.id);
    assert(record.revision === 1 && record.committedBytes === 0n);
    await lease.close();
  }),
  'cancel during committed IO cannot publish a late checkpoint update': async () => fixture(async f => {
    let release!: () => void; let entered!: () => void;
    const gate = new Promise<void>(resolve => { release = resolve; });
    const started = new Promise<void>(resolve => { entered = resolve; });
    const destination: ContentDestination = { targetId: f.entry.targetId, async open(options) {
      const writer = await f.entry.destination.open(options);
      return { ...writer, async commit() { await writer.commit(); entered(); await gate; } };
    }};
    const session = await f.resume({ destination });
    await session.writeBlock(0n, f.block(0));
    const committing = rejects(session.commit(), 'TASK_CANCELLED');
    await started;
    const aborting = session.abort();
    release();
    await Promise.all([committing, aborting]);
    assert((await f.state.read(f.checkpoint.id)).committedBytes === 0n);
  }),
  'finished target can be revalidated and finalized after interrupted result delivery': async () => fixture(async f => {
    const first = await f.resume();
    for (let i = 0; i < 3; i++) await first.writeBlock(BigInt(i), f.block(i));
    await first.finish();
    const next = await f.resume();
    assert(next.snapshot().committedBytes === BigInt(f.bytes.length));
    await next.finish();
    assert(await f.entry.status() === 'complete');
  }),
  'unknown schema and corrupt bitmap are rejected without silently resetting': async () => fixture(async f => {
    await mutate(f.namespace, f.checkpoint.id, record => { record.version = 99; });
    await rejects(f.state.read(f.checkpoint.id), 'CHECKPOINT_VERSION');
    await mutate(f.namespace, f.checkpoint.id, record => { record.version = 1; record.committed = new Uint8Array([128]); });
    await rejects(f.state.read(f.checkpoint.id), 'CHECKPOINT_CORRUPT');
    await f.state.discard(f.checkpoint.id);
    assert(await f.entry.status() === 'partial');
  }),
  'state close releases leases and rejects later operations': async () => fixture(async f => {
    const lease = await f.state.lease(f.checkpoint.id);
    const closing = f.state.close();
    assert(f.state.close() === closing);
    await closing;
    assert(lease.closed);
    await rejects(f.state.read(f.checkpoint.id), 'STATE_CLOSED');
    const other = await browserState({ namespace: f.namespace });
    try { const next = await other.lease(f.checkpoint.id); await next.close(); }
    finally { await other.close(); }
  })
};

let live: Awaited<ReturnType<typeof makeFixture>> | undefined;
export async function prepareReload() {
  live = await makeFixture();
  const session = await live.resume();
  await session.writeBlock(0n, live.block(0));
  await session.commit();
  // Leave the native worker + checkpoint lease active across navigation.
  return { namespace: live.namespace, checkpointId: live.checkpoint.id, cacheId: live.entry.id,
    seed: Array.from(live.seed), proof: Array.from(live.proof) };
}
export async function checkReload(options: Awaited<ReturnType<typeof prepareReload>>) {
  const state = await browserState({ namespace: options.namespace });
  const cache = await opfsCache();
  const entry = await cache.open(options.cacheId);
  const verifier = await createBlockVerifier({ seed: new Uint8Array(options.seed), proof: new Uint8Array(options.proof) });
  let session: CheckpointSession | undefined;
  try {
    session = await resumeCheckpoint({ state, checkpointId: options.checkpointId, verifier, destination: entry.destination });
    assert(session.snapshot().committedBytes === BigInt(BLOCK));
    await session.abort();
    await state.discard(options.checkpointId);
  } finally { await session?.abort(); await state.close(); await cache.remove(entry.id); verifier.close(); }
}

export async function contend(namespace: string, id: string) {
  const state = await browserState({ namespace });
  try {
    await rejects(state.lease(id), 'CHECKPOINT_BUSY');
    await rejects(state.discard(id), 'CHECKPOINT_BUSY');
  } finally { await state.close(); }
}
