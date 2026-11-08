import init, { SeedBuilder } from '@q2q/wasm';
import { browserState, opfsCache, resumeCheckpoint, createBlockVerifier, fileDestination, fileSource, StorageError } from '../src/index.ts';
import type { ContentSource, CheckpointSession, StateStore, ContentVerifier, OpfsCache, OpfsEntry } from '../src/index.ts';

const BLOCK = 262144;
function assert(value: unknown, message: string): asserts value { if (!value) throw new Error(message); }
function chunk(index: number) {
  const bytes = new Uint8Array(BLOCK);
  for (let offset = 0; offset < bytes.length; offset++) bytes[offset] = offset % 251;
  new DataView(bytes.buffer).setUint32(0, index, true);
  return bytes;
}
function hex(bytes: Uint8Array) { return Array.from(bytes, value => value.toString(16).padStart(2, '0')).join(''); }
function phase(value: string) { console.log(`C5-PHASE ${value}`); }

interface Descriptor {
  mib: number; namespace: string; checkpointId: string; cacheId: string;
  seed: number[]; proof: number[]; infoHash: string; quarter: number;
  seedMs: number; quarterWriteMs: number;
}
interface Resources {
  state: StateStore; verifier: ContentVerifier; cache: OpfsCache; entry: OpfsEntry;
  session?: CheckpointSession; exportName?: string;
}
let resources: Resources | undefined;

export async function prepare(mib: number): Promise<Descriptor> {
  assert(mib === 100 || mib === 1024, 'only the planned 100 MiB / 1 GiB cases are allowed');
  await init();
  const size = mib * 1024 * 1024;
  phase('build-seed');
  const seedStart = performance.now();
  const builder = new SeedBuilder(size);
  for (let index = 0; index < size / BLOCK; index++) builder.update(chunk(index));
  const prepared = builder.finish('benchmark.bin');
  const seed = prepared.seed_bytes(); const proof = prepared.proof_bytes();
  const infoHash = hex(prepared.info_hash()); prepared.free();
  const verifier = await createBlockVerifier({ seed, proof });
  assert(verifier.content.blockSize === BLOCK, 'benchmark fixture block size changed');
  const seedMs = performance.now() - seedStart;
  const namespace = `large-${crypto.randomUUID()}`;
  const state = await browserState({ namespace });
  const cache = await opfsCache();
  const entry = await cache.create({ name: 'benchmark.bin', size: BigInt(size) });
  const checkpoint = await state.create({ content: verifier.content, targetId: entry.targetId });
  resources = { state, verifier, cache, entry };
  const session = await resumeCheckpoint({ state, verifier, checkpointId: checkpoint.id, destination: entry.destination });
  resources.session = session;
  const quarter = verifier.blockCount / 4;
  phase('write-quarter');
  const writeStart = performance.now();
  for (let cursor = 0; cursor < quarter; cursor++) {
    // Pair-reversed arrival tests positional writes without buffering all blocks.
    const index = cursor ^ 1;
    await session.writeBlock(BigInt(index), chunk(index));
    if ((cursor + 1) % 64 === 0) await session.commit();
  }
  await session.commit();
  assert(session.snapshot().committedBytes === BigInt(size / 4), 'quarter progress mismatch');
  // Keep both leases and the Worker alive: the runner forces a real reload.
  return { mib, namespace, checkpointId: checkpoint.id, cacheId: entry.id,
    seed: Array.from(seed), proof: Array.from(proof), infoHash, quarter,
    seedMs, quarterWriteMs: performance.now() - writeStart };
}

async function verifySource(source: ContentSource, expected: string) {
  const reader = await source.open();
  const builder = new SeedBuilder(Number(source.info.size));
  try {
    for (let offset = 0n; offset < source.info.size; offset += 1048576n) {
      const length = Number(source.info.size - offset < 1048576n ? source.info.size - offset : 1048576n);
      builder.update(await reader.read(offset, length));
    }
    const prepared = builder.finish('benchmark.bin');
    try { assert(hex(prepared.info_hash()) === expected, 'final bytes do not match original Q2Q seed'); }
    finally { prepared.free(); }
  } finally { await reader.close(); }
}

export async function resume(descriptor: Descriptor) {
  const { namespace, checkpointId, cacheId, quarter, infoHash } = descriptor;
  const state = await browserState({ namespace });
  const cache = await opfsCache();
  const entry = await cache.open(cacheId);
  const verifier = await createBlockVerifier({ seed: new Uint8Array(descriptor.seed), proof: new Uint8Array(descriptor.proof) });
  resources = { state, verifier, cache, entry };
  phase('revalidate-quarter');
  const resumeStart = performance.now();
  const session = await resumeCheckpoint({ state, checkpointId, verifier, destination: entry.destination });
  resources.session = session;
  assert(session.snapshot().committedBytes === BigInt(quarter * BLOCK), 'revalidated progress mismatch');
  const revalidateMs = performance.now() - resumeStart;
  phase('write-rest');
  const restStart = performance.now();
  for (let cursor = quarter; cursor < verifier.blockCount; cursor++) {
    const index = cursor ^ 1;
    await session.writeBlock(BigInt(index), chunk(index));
    if ((cursor + 1) % 64 === 0) await session.commit();
  }
  await session.finish();
  const writeRestMs = performance.now() - restStart;
  phase('verify-cache');
  const verifyStart = performance.now();
  await verifySource(entry.source, infoHash);
  const cacheVerifyMs = performance.now() - verifyStart;
  const root = await navigator.storage.getDirectory();
  const exportName = `q2q-large-export-${crypto.randomUUID()}`;
  resources.exportName = exportName;
  const handle = await root.getFileHandle(exportName, { create: true });
  phase('export');
  const exportStart = performance.now();
  const receipt = await entry.exportTo(await fileDestination(handle));
  assert(receipt.kind === 'staged', 'OPFS export must not pretend to be a user file');
  const exportMs = performance.now() - exportStart;
  phase('verify-export');
  const finalVerifyStart = performance.now();
  await verifySource(fileSource(await handle.getFile()), infoHash);
  const exportVerifyMs = performance.now() - finalVerifyStart;
  phase('complete');
  return { mib: descriptor.mib, seedMs: Math.round(descriptor.seedMs),
    quarterWriteMs: Math.round(descriptor.quarterWriteMs), revalidateMs: Math.round(revalidateMs),
    writeRestMs: Math.round(writeRestMs), cacheVerifyMs: Math.round(cacheVerifyMs),
    exportMs: Math.round(exportMs), exportVerifyMs: Math.round(exportVerifyMs),
    bytes: entry.size.toString(), originalSeedMatchesCacheAndExport: true };
}

export async function cleanup() {
  const current = resources;
  resources = undefined;
  if (!current) return;
  try {
    await current.session?.abort();
    const records = await current.state.list();
    for (const record of records.items) await current.state.discard(record.id);
    await current.cache.remove(current.entry.id);
    if (current.exportName) await (await navigator.storage.getDirectory()).removeEntry(current.exportName);
  } finally { await current.state.close(); current.verifier.close(); }
}

export async function quotaCase() {
  const cache = await opfsCache();
  const entry = await cache.create({ name: 'quota.bin', size: 32n * 1024n * 1024n });
  let writer: Awaited<ReturnType<typeof entry.destination.open>> | undefined;
  let written = 0;
  try {
    writer = await entry.destination.open({ mode: 'create', size: entry.size });
    const bytes = chunk(0);
    for (let index = 0; index < 128; index++) {
      await writer.write(BigInt(index * BLOCK), bytes);
      written += BLOCK;
      await writer.commit();
    }
    throw new Error('quota case unexpectedly completed');
  } catch (error) {
    assert(error instanceof StorageError && error.code === 'STORAGE_FULL', 'quota must produce STORAGE_FULL');
    await writer?.abort();
    assert(await entry.status() === 'partial', 'quota failure must not finalize cache');
    return { error: error.code, acceptedBytesBeforeFailure: written, status: 'partial' };
  } finally { await writer?.abort(); await cache.remove(entry.id); }
}
