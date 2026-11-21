import { SeedBuilder, seed_with_source } from '@q2q/wasm';
import { initializeWasm } from '../src/block-verifier.ts';
import { createClient, opfsCache, browserState, Q2QError } from '../src/index.ts';
import type { StreamReader } from '../src/index.ts';
const BLOCK = 262144;
function assert(value: unknown, message = 'assertion failed'): asserts value { if (!value) throw new Error(message); }
async function rejects(promise: Promise<unknown>, code: string) {
  try { await promise; } catch (error) { assert(error instanceof Q2QError && error.code === code, `expected ${code}, got ${String(error)}`); return; }
  throw new Error(`expected ${code}`);
}
async function prepare(settings: { blocks?: number; kind?: string; peerOnly?: boolean } = {}) {
  await initializeWasm();
  const blocks = settings.blocks ?? 4;
  const size = (blocks - 1) * BLOCK + 1000;
  const bytes = new Uint8Array(size);
  for (let i = 0; i < size; i++) bytes[i] = (i * 31 + Math.floor(i / BLOCK)) % 251;
  const builder = new SeedBuilder(size); builder.update(bytes);
  const material = settings.peerOnly ? builder.finish('stream.bin') : builder.finish_inline('stream.bin');
  let raw = material.seed_bytes(); const proof = material.proof_bytes(); material.free();
  const key = crypto.randomUUID();
  const base = location.origin;
  const url = (kind: string) => `${base}/fixture/${kind}?key=${key}&size=${size}`;
  await fetch(url('register'), { method: 'POST', body: proof as Uint8Array<ArrayBuffer> });
  // A peer-only seed carries neither inline leaf hashes nor a proof source.
  const seedBytes = settings.peerOnly ? raw : seed_with_source(raw, url(settings.kind ?? 'good'));
  const state = await browserState({ namespace: crypto.randomUUID() });
  const client = createClient({ state });
  const cache = await opfsCache();
  const seed = await client.parseSeed(new Uint8Array(seedBytes));
  const entry = await cache.create({ name: 'stream.bin', size: seed.info.size });
  return { client, state, cache, seed, entry, key, base, bytes, url };
}
async function ranges(key: string) {
  const response = await fetch(`${location.origin}/fixture/stats?key=${key}`);
  const body = JSON.parse(await response.text()) as { records: { kind: string; range: string | undefined }[] };
  return body.records.filter(record => record.kind === 'good' || record.kind === 'slow' || record.kind === 'stall')
    .map(record => /^bytes=(\d+)-/.exec(record.range ?? '')?.[1]).filter((value): value is string => value !== undefined).map(Number);
}
async function cleanup(f: Awaited<ReturnType<typeof prepare>>) {
  await f.client.close().catch(() => {});
  await f.state.close().catch(() => {});
  await f.cache.remove(f.entry.id).catch(() => {});
}

/** Reading the head must fetch only the cursor block when prefetch is 0. */
export async function sequentialHeadRead() {
  const f = await prepare();
  let reader: StreamReader | undefined;
  try {
    reader = await f.client.openStream({ seed: f.seed, destination: f.entry.destination, prefetchBlocks: 0 });
    const head = await reader.read(1024);
    assert(head.length === 1024, `head length ${head.length}`);
    assert(head.every((byte, i) => byte === f.bytes[i]), 'head bytes match source');
    assert(reader.position === 1024n, `position ${reader.position}`);
    const fetched = await ranges(f.key);
    assert(fetched.every(offset => offset === 0), `prefetch 0 fetched only block 0, got ${JSON.stringify(fetched)}`);

    // A read never crosses a block boundary; the tail of block 0 is short.
    const rest = await reader.read(BLOCK);
    assert(rest.length === BLOCK - 1024, `block boundary clamp ${rest.length}`);
    assert(rest.every((byte, i) => byte === f.bytes[1024 + i]), 'rest bytes match source');
    return true;
  } finally { await reader?.close(); await cleanup(f); }
}

/** Prefetch stays bounded to the window and a seek moves it. */
export async function windowAndSeek() {
  const f = await prepare();
  let reader: StreamReader | undefined;
  try {
    reader = await f.client.openStream({ seed: f.seed, destination: f.entry.destination, prefetchBlocks: 2 });
    await reader.read(16);
    await new Promise(resolve => setTimeout(resolve, 300));
    // Offsets, not block indices: block n starts at n * BLOCK.
    let fetched = new Set(await ranges(f.key));
    assert(fetched.has(0), 'block 0 fetched');
    assert(!fetched.has(3 * BLOCK), `block 3 is outside the window, got ${JSON.stringify([...fetched])}`);

    // Seek to the last block: only that block's window is needed now.
    const lastOffset = BigInt(3 * BLOCK + 100);
    await reader.seek(lastOffset);
    const tail = await reader.read(64);
    assert(tail.every((byte, i) => byte === f.bytes[Number(lastOffset) + i]), 'tail bytes match source');
    fetched = new Set(await ranges(f.key));
    assert(fetched.has(3 * BLOCK), `block 3 fetched after seek; offsets=${JSON.stringify([...fetched])}`);
    return true;
  } finally { await reader?.close(); await cleanup(f); }
}

/** A stalled block times out without moving the cursor; retry succeeds after. */
export async function readTimeoutThenRetry() {
  const f = await prepare({ kind: 'stall' });
  let reader: StreamReader | undefined;
  try {
    reader = await f.client.openStream({ seed: f.seed, destination: f.entry.destination, prefetchBlocks: 0, readTimeoutMs: 400 });
    await rejects(reader.read(16), 'CONTROL_TIMEOUT');
    assert(reader.position === 0n, 'timeout must not advance the cursor');
    return true;
  } finally { await reader?.close(); await cleanup(f); }
}

/** Closing keeps verified blocks, and reopening resumes without refetching. */
export async function closePreservesVerifiedBlocks() {
  const f = await prepare();
  let reader: StreamReader | undefined;
  let reopened: StreamReader | undefined;
  try {
    reader = await f.client.openStream({ seed: f.seed, destination: f.entry.destination, prefetchBlocks: 0 });
    await reader.read(1024);
    const checkpointId = reader.checkpointId;
    const committed = reader.progress().committedBytes;
    assert(committed >= 1024n, `committed ${committed}`);
    await reader.close();

    const before = (await ranges(f.key)).length;
    reopened = await f.client.openStream({ seed: f.seed, destination: f.entry.destination, prefetchBlocks: 0, checkpointId });
    const again = await reopened.read(1024);
    assert(again.every((byte, i) => byte === f.bytes[i]), 'resumed bytes match');
    assert((await ranges(f.key)).length === before, 'committed block was not refetched');
    assert(reopened.progress().committedBytes >= committed, 'committed progress preserved');
    return true;
  } finally { await reopened?.close(); await reader?.close(); await cleanup(f); }
}

/** Peer-only content has no proof table to re-verify reads, so it is rejected. */
export async function peerOnlyRejected() {
  const f = await prepare({ peerOnly: true });
  try {
    await rejects(f.client.openStream({ seed: f.seed, destination: f.entry.destination }), 'PROOF_UNAVAILABLE');
    return true;
  } finally { await cleanup(f); }
}

/** Out-of-range seek and non-positive reads are rejected before touching IO. */
export async function argumentBounds() {
  const f = await prepare();
  let reader: StreamReader | undefined;
  try {
    reader = await f.client.openStream({ seed: f.seed, destination: f.entry.destination });
    await rejects(reader.seek(-1n), 'INVALID_RANGE');
    await rejects(reader.seek(f.seed.info.size + 1n), 'INVALID_RANGE');
    await rejects(reader.read(0), 'INVALID_RANGE');
    await rejects(f.client.openStream({ seed: f.seed, destination: f.entry.destination, prefetchBlocks: 65 }), 'INVALID_ARGUMENT');
    await rejects(f.client.openStream({ seed: f.seed, destination: f.entry.destination, readTimeoutMs: 0 }), 'INVALID_ARGUMENT');
    // Seeking to EOF is legal and reads return empty without prefetching.
    await reader.seek(f.seed.info.size);
    assert((await reader.read(16)).length === 0, 'read at EOF is empty');
    return true;
  } finally { await reader?.close(); await cleanup(f); }
}

/** Streaming from a live peer: read the head, then seek near the tail. Content
 * is served by the native publisher, so blocks really arrive over Track/Punch. */
export async function p2pStream(fixture: { seed: string; trackUrl: string }) {
  const client = createClient({ trackUrl: fixture.trackUrl });
  const cache = await opfsCache();
  let reader: StreamReader | undefined;
  try {
    const seed = await client.parseSeed(Uint8Array.from(atob(fixture.seed), c => c.charCodeAt(0)));
    const entry = await cache.create({ name: 'stream-p2p.bin', size: seed.info.size });
    try {
      reader = await client.openStream({ seed, destination: entry.destination, prefetchBlocks: 1 });
      const size = Number(seed.info.size);
      const expected = (offset: number) => (offset * 31 + Math.floor(offset / BLOCK)) % 251;
      const head = await reader.read(1024);
      assert(head.length === 1024, `head length ${head.length}`);
      assert(head.every((byte, i) => byte === expected(i)), 'head bytes match publisher');
      // A seek makes a distant block available on demand. Reads never cross a
      // block boundary, so stay inside the block that contains this offset.
      const tailOffset = size - 100;
      await reader.seek(BigInt(tailOffset));
      const tail = await reader.read(32);
      assert(tail.length === 32, `tail length ${tail.length}`);
      assert(tail.every((byte, i) => byte === expected(tailOffset + i)), 'tail bytes match publisher');
      // The final short block (17 bytes) is readable too.
      const last = size - 17;
      await reader.seek(BigInt(last));
      const end = await reader.read(64);
      assert(end.length === 17, `final block length ${end.length}`);
      assert(end.every((byte, i) => byte === expected(last + i)), 'final bytes match publisher');
      return { committed: reader.progress().committedBytes, size };
    } finally { await cache.remove(entry.id).catch(() => {}); }
  } finally { await reader?.close(); await client.close(); }
}
