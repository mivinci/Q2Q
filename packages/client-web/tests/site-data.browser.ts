import init, { SeedBuilder } from '@q2q/wasm';
import { browserState, opfsCache, resumeCheckpoint, createBlockVerifier, StorageError } from '../src/index.ts';
import type { StateStore, CheckpointSession, ContentVerifier } from '../src/index.ts';

let resources: { state: StateStore; session: CheckpointSession; verifier: ContentVerifier; checkpointId: string; cacheId: string } | undefined;
export async function prepare() {
  await init();
  const bytes = new Uint8Array([97, 98, 99]);
  const builder = new SeedBuilder(3); builder.update(bytes);
  const material = builder.finish('cleared.bin');
  const verifier = await createBlockVerifier({ seed: material.seed_bytes(), proof: material.proof_bytes() }); material.free();
  const state = await browserState({ namespace: 'clear-data-test' });
  const cache = await opfsCache(); const entry = await cache.create({ name: 'cleared.bin', size: 3n });
  const checkpoint = await state.create({ content: verifier.content, targetId: entry.targetId });
  const session = await resumeCheckpoint({ state, checkpointId: checkpoint.id, verifier, destination: entry.destination });
  resources = { state, session, verifier, checkpointId: checkpoint.id, cacheId: entry.id };
  await session.writeBlock(0n, bytes); await session.commit();
}
export async function check() {
  if (!resources) throw new Error('missing clear-data test');
  const { state, session, verifier, checkpointId, cacheId } = resources;
  try {
    try { await session.writeBlock(0n, new Uint8Array([97, 98, 99])); throw new Error('cleared session accepted a write'); }
    catch (error) { if (!(error instanceof StorageError) || error.code !== 'STATE_CLOSED') throw error; }
    await session.abort();
    const fresh = await browserState({ namespace: 'clear-data-test' });
    try {
      if ((await fresh.list()).items.length !== 0) throw new Error('cleared checkpoints reappeared');
      try { await fresh.read(checkpointId); throw new Error('old checkpoint survived'); }
      catch (error) { if (!(error instanceof StorageError) || error.code !== 'CHECKPOINT_NOT_FOUND') throw error; }
    } finally { await fresh.close(); }
    const cache = await opfsCache();
    try { await cache.open(cacheId); throw new Error('old cache survived'); }
    catch (error) { if (!(error instanceof StorageError) || error.code !== 'CACHE_NOT_FOUND') throw error; }
  } finally { await session.abort(); await state.close(); verifier.close(); resources = undefined; }
}

export async function stopState() {
  if (!resources) throw new Error('missing clear-data test');
  await resources.state.close();
}
export async function ioStopped() {
  if (!resources) throw new Error('missing clear-data test');
  try {
    const entry = await (await opfsCache()).open(resources.cacheId);
    return await entry.status() === 'partial';
  } catch (error) {
    if (error instanceof StorageError && error.code === 'DESTINATION_BUSY') return false;
    throw error;
  }
}
