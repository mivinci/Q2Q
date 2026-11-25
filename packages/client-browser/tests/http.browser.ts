import { SeedBuilder, seed_with_source } from '@q2q/wasm';
import { initializeWasm } from '../src/block-verifier.ts';
import { createClient, opfsCache, browserState, Q2QError } from '../src/index.ts';
import type { ClientOptions, Transfer, DownloadSnapshot } from '../src/index.ts';
const BLOCK = 262144;
function assert(value: unknown, message = 'assertion failed'): asserts value { if (!value) throw new Error(message); }
async function rejects(promise: Promise<unknown>, code: string) {
  try { await promise; } catch (error) { assert(error instanceof Q2QError && error.code === code, `expected ${code}, got ${String(error)}`); return; }
  throw new Error(`expected ${code}`);
}
async function fixture(run: (f: Awaited<ReturnType<typeof prepare>>) => Promise<void>, settings: Parameters<typeof prepare>[0] = {}) {
  const f = await prepare(settings);
  try { await run(f); } finally { await f.client.close(); await f.state?.close(); await f.cache.remove(f.entry.id); }
}
async function prepare(settings: { size?: number; kind?: string; inline?: boolean; cross?: boolean; options?: ClientOptions } = {}) {
  await initializeWasm();
  const bytes = new Uint8Array(settings.size ?? 2 * BLOCK + 17);
  for (let i = 0; i < bytes.length; i++) bytes[i] = (i * 31 + Math.floor(i / BLOCK)) % 251;
  const builder = new SeedBuilder(bytes.length); builder.update(bytes);
  const material = settings.inline === false ? builder.finish('download.bin') : builder.finish_inline('download.bin');
  let raw = material.seed_bytes(); const proof = material.proof_bytes(); material.free();
  const key = crypto.randomUUID();
  const base = settings.cross ? location.origin.replace('127.0.0.1', 'localhost') : location.origin;
  const url = (kind: string) => `${base}/fixture/${kind}?key=${key}&size=${bytes.length}`;
  await fetch(url('register'), { method: 'POST', body: proof as Uint8Array<ArrayBuffer> });
  if (settings.inline !== false) raw = seed_with_source(raw, url(settings.kind ?? 'good'));
  const state = settings.options?.state;
  const client = createClient({ ...settings.options,
    ...(settings.inline === false && !settings.options?.sourceProvider ? { sourceProvider: { async resolve() { return { contentUrls: [url(settings.kind ?? 'good')], proofUrls: [url('proof')] }; } } } : {}) });
  const seed = await client.parseSeed(raw);
  const cache = await opfsCache(); const entry = await cache.create({ name: 'download.bin', size: seed.info.size });
  return { client, seed, raw, cache, entry, bytes, url, state,
    start: () => client.download({ seed, destination: entry.destination }),
    stats: async () => (await (await fetch(url('stats'))).json()) as { records: Array<{kind: string; range?: string; cookie?: string; referer?: string}>; peak: number },
    async check() {
      assert(await entry.status() === 'complete'); const reader = await entry.source.open();
      try { const read = await reader.read(0n, bytes.length); assert(read.length === bytes.length && read.every((v, i) => v === bytes[i]), 'download bytes differ'); }
      finally { await reader.close(); }
    }
  };
}
async function until(task: Transfer, predicate: (s: Readonly<DownloadSnapshot>) => boolean) {
  if (predicate(task.snapshot())) return;
  await new Promise<void>((resolve, reject) => {
    const timeout = setTimeout(() => { off(); reject(new Error(`state timeout ${task.snapshot().state}`)); }, 10000);
    const off = task.subscribe(s => { if (predicate(s)) { clearTimeout(timeout); off(); resolve(); } });
  });
}
export const cases: Record<string, () => Promise<void>> = {
  'connection diagnostic survives rejection and failed task snapshot': () => fixture(async f => {
    const task=f.start();
    try {await task.wait();throw new Error('expected failure');}
    catch(error){assert(error instanceof Q2QError && error.code==='CONTROL_TIMEOUT');assert(error.diagnostic?.stage==='rtc-connect');}
    const error=task.snapshot().error;assert(task.snapshot().state==='failed');
    assert(error?.code==='CONTROL_TIMEOUT' && error.diagnostic?.iceConnectionState==='checking');
    assert(Object.isFrozen(error) && Object.isFrozen(error.diagnostic));
  }, {options:{sourceProvider:{async resolve(){throw new Q2QError('CONTROL_TIMEOUT',true,{stage:'rtc-connect',iceConnectionState:'checking',localCandidates:2,remoteCandidates:1});}}}}),
  'inline seed download verifies and saves real OPFS bytes': () => fixture(async f => {
    const task = f.start(); const states: string[] = [];
    task.subscribe(s => { states.push(s.state); });
    task.subscribe(() => { throw new Error('observer'); });
    const [a, b] = await Promise.all([task.wait(), task.wait()]);
    assert(a.content.id === b.content.id && a.output.kind === 'staged');
    assert(task.snapshot().verifiedBytes === BigInt(f.bytes.length) && task.snapshot().committedBytes === BigInt(f.bytes.length));
    assert(states.includes('downloading') && states.includes('finalizing') && states.at(-1) === 'completed');
    assert(!JSON.stringify(task.snapshot(), (_, v) => typeof v === 'bigint' ? v.toString() : v).includes('/fixture/'));
    await f.check();
    const { records } = await f.stats(); assert(records.length === 3 && records.every(r => !r.cookie && !r.referer));
  }),
  'external proof is fetched and checked with shared Wasm': () => fixture(async f => { await f.start().wait(); await f.check(); assert((await f.stats()).records.some(r => r.kind === 'proof')); }, { inline: false }),
  'redirect source remains content verified': () => fixture(async f => { await f.start().wait(); await f.check(); }, { kind: 'redirect' }),
  'single block accepts whole file 200 response': () => fixture(async f => { await f.start().wait(); await f.check(); }, { kind: 'full', size: 37 }),
  'empty content completes without content requests': () => fixture(async f => { await f.start().wait(); await f.check(); assert((await f.stats()).records.length === 0); }, { size: 0, inline: false }),
  'cross origin Range works with exposed Content-Range': () => fixture(async f => { await f.start().wait(); await f.check(); }, { cross: true }),
  'hidden cross origin Content-Range is rejected': () => fixture(async f => { await rejects(f.start().wait(), 'HTTP_PROTOCOL_ERROR'); assert(await f.entry.status() === 'partial'); }, { cross: true, kind: 'hidden' }),
  '503 is bounded and retried': () => fixture(async f => { await f.start().wait(); await f.check(); assert((await f.stats()).records.length === 4); }, { kind: 'retry', options: { limits: { httpRequests: 1, retryDelayMs: 1 } } }),
  'retry exhaustion fails without completion': () => fixture(async f => { const t = f.start(); await rejects(t.wait(), 'HTTP_STATUS'); assert((await f.stats()).records.length === 2); assert(t.snapshot().committedBytes === 0n); }, { kind: 'unavailable', options: { limits: { httpRequests: 1, retryDelayMs: 1 } } }),
  'request deadline covers stalled response body': () => fixture(async f => { await rejects(f.start().wait(), 'NETWORK_UNAVAILABLE'); assert(await f.entry.status() === 'partial'); }, { kind: 'stall', options: { limits: { httpRequests: 1, attemptsPerSource: 1, requestTimeoutMs: 80 } } }),
  'pause commits verified blocks and resume fetches only missing ranges': () => fixture(async f => {
    const task = f.start(); await until(task, s => s.verifiedBytes >= BigInt(BLOCK));
    await task.pause(); await task.pause(); const paused = task.snapshot(); assert(paused.state === 'paused' && paused.committedBytes >= BigInt(BLOCK));
    const count = (await f.stats()).records.length;
    await Promise.all([task.resume(), task.resume()]); await task.wait(); await f.check();
    const subsequent = (await f.stats()).records.slice(count); assert(subsequent.every(r => r.range !== `bytes=0-${BLOCK - 1}`));
  }, { kind: 'slow', options: { limits: { httpRequests: 1, commitBytes: BLOCK } } }),
  'fresh client revalidates checkpoint and resumes': async () => {
    const state = await browserState({ namespace: crypto.randomUUID() });
    await fixture(async f => {
      const task = f.start(); await until(task, s => s.verifiedBytes >= BigInt(BLOCK)); await task.pause();
      const checkpointId = task.snapshot().checkpointId!; await f.client.close();
      const client = createClient({ state, limits: { httpRequests: 1 } });
      try { const seed = await client.parseSeed(f.raw); await client.resumeDownload({ seed, destination: f.entry.destination, checkpointId }).wait(); await f.check(); }
      finally { await client.close(); }
      assert((await state.read(checkpointId)).committedBytes === BigInt(f.bytes.length));
    }, { kind: 'slow', options: { state, limits: { httpRequests: 1, commitBytes: BLOCK } } });
  },
  'cancel drains IO and releases destination without waiting observer': () => fixture(async f => {
    const task = f.start(); await until(task, s => s.state === 'downloading'); await task.cancel();
    await rejects(task.wait(), 'TASK_CANCELLED'); assert(task.snapshot().state === 'cancelled');
    const writer = await f.entry.destination.open({ mode: 'resume', size: f.seed.info.size }); await writer.abort();
    await rejects(task.resume(), 'INVALID_STATE');
  }, { kind: 'slow' }),
  'cancel before task starts does not create a checkpoint': () => fixture(async f => {
    const task = f.start(); await task.cancel(); await rejects(task.wait(), 'TASK_CANCELLED'); assert((await f.client.listCheckpoints()).items.length === 0);
  }),
  'client close is idempotent and preserves injected state': async () => {
    const state = await browserState({ namespace: crypto.randomUUID() });
    await fixture(async f => {
      const task = f.start(); await until(task, s => s.state === 'downloading');
      const a = f.client.close(); assert(a === f.client.close()); await a; await rejects(task.wait(), 'TASK_CANCELLED');
      await state.list(); await rejects(Promise.resolve().then(() => f.client.parseSeed(f.raw)), 'CLIENT_CLOSED');
    }, { kind: 'slow', options: { state } });
  },
  'parseSeed copies bytes and does not fetch hints': () => fixture(async f => {
    const seed = await f.client.parseSeed(f.raw); const copy = seed.toBytes(); copy.fill(0);
    assert(seed.toBytes().some(v => v !== 0) && (await f.stats()).records.length === 0);
    await rejects(f.client.parseSeed(new Uint8Array([0])), 'INVALID_SEED');
  }),
  'concurrent 401 uses one credential refresh without exposing URLs': () => fixture(async f => {
    const reasons: string[] = [];
    const client = createClient({ sourceProvider: { async resolve({ reason }) {
      reasons.push(reason); if (reason === 'unauthorized') await new Promise(resolve => setTimeout(resolve, 60));
      return { contentUrls: [f.url(reason === 'initial' ? 'auth' : 'good')] };
    } } });
    try { const seed = await client.parseSeed(f.raw); await client.download({ seed, destination: f.entry.destination }).wait(); await f.check();
      assert(reasons.join(',') === 'initial,unauthorized', reasons.join(',')); }
    finally { await client.close(); }
  }),
  'repeated 401 does not loop refresh': () => fixture(async f => {
    let count = 0;
    const client = createClient({ limits: { httpRequests: 1 }, sourceProvider: { async resolve() { count++; return { contentUrls: [f.url('auth')] }; } } });
    try { const seed = await client.parseSeed(f.raw); await rejects(client.download({ seed, destination: f.entry.destination }).wait(), 'AUTH_REQUIRED'); assert(count === 2); }
    finally { await client.close(); }
  }),
  'corrupt source falls back to a verified origin': () => fixture(async f => {
    const client = createClient({ sourceProvider: { async resolve() { return { contentUrls: [f.url('corrupt'), f.url('good')] }; } } });
    try { const seed = await client.parseSeed(f.raw); const task = client.download({ seed, destination: f.entry.destination }); await task.wait(); await f.check(); assert(task.snapshot().receivedBytes > seed.info.size); }
    finally { await client.close(); }
  }),
  'corrupt proof falls back before any content progress': () => fixture(async f => {
    const client = createClient({ sourceProvider: { async resolve() { return { contentUrls: [f.url('good')], proofUrls: [f.url('badproof'), f.url('proof')] }; } } });
    try { const seed = await client.parseSeed(f.raw); await client.download({ seed, destination: f.entry.destination }).wait(); await f.check(); }
    finally { await client.close(); }
  }, { inline: false }),
  'invalid proof cannot create a checkpoint': () => fixture(async f => {
    const client = createClient({ sourceProvider: { async resolve() { return { contentUrls: [f.url('good')], proofUrls: [f.url('badproof')] }; } } });
    try { const seed = await client.parseSeed(f.raw); await rejects(client.download({ seed, destination: f.entry.destination }).wait(), 'CONTENT_MISMATCH'); assert((await client.listCheckpoints()).items.length === 0); }
    finally { await client.close(); }
  }, { inline: false }),
  'cancel ignores late provider resolution and never restarts IO': () => fixture(async f => {
    let ready!: () => void; const called = new Promise<void>(resolve => { ready = resolve; });
    let supply!: (value: { contentUrls: string[] }) => void;
    const client = createClient({ sourceProvider: { async resolve() { ready(); return new Promise(resolve => { supply = resolve; }); } } });
    try { const seed = await client.parseSeed(f.raw); const task = client.download({ seed, destination: f.entry.destination });
      await called; await task.cancel(); supply({ contentUrls: [f.url('good')] }); await rejects(task.wait(), 'TASK_CANCELLED');
      await new Promise(resolve => setTimeout(resolve, 60)); assert((await f.stats()).records.length === 0); }
    finally { await client.close(); }
  }),
  'multiple tasks share the client HTTP request budget': () => fixture(async f => {
    const client = createClient({ limits: { httpRequests: 1 } });
    const other = await f.cache.create({ name: 'other.bin', size: f.seed.info.size });
    try { const seed = await client.parseSeed(f.raw); const a = client.download({ seed, destination: f.entry.destination }); const b = client.download({ seed, destination: other.destination });
      await Promise.all([a.wait(), b.wait()]); await f.check(); assert((await f.stats()).peak === 1); }
    finally { await client.close(); await f.cache.remove(other.id); }
  }, { kind: 'slow' }),
  'resume rejects a different destination identity': async () => {
    const state = await browserState({ namespace: crypto.randomUUID() });
    await fixture(async f => {
      const task = f.start(); await until(task, s => s.verifiedBytes >= BigInt(BLOCK)); await task.pause();
      const other = await f.cache.create({ name: 'other.bin', size: f.seed.info.size });
      try { await rejects(f.client.resumeDownload({ seed: f.seed, destination: other.destination, checkpointId: task.snapshot().checkpointId! }).wait(), 'CHECKPOINT_MISMATCH'); }
      finally { await f.cache.remove(other.id); }
    }, { kind: 'slow', options: { state, limits: { httpRequests: 1 } } });
  },
  'maxTasks bounds nonterminal handles including paused': () => fixture(async f => {
    const task = f.start(); await task.pause();
    try { f.start(); throw new Error('expected limit'); } catch (error) { assert(error instanceof Q2QError && error.code === 'RESOURCE_LIMIT'); }
    await task.cancel();
  }, { options: { limits: { maxTasks: 1 } } }),
};
for (const [kind, code] of Object.entries({ full: 'HTTP_RANGE_UNSUPPORTED', badrange: 'HTTP_PROTOCOL_ERROR', changed: 'SOURCE_CHANGED', corrupt: 'CONTENT_MISMATCH', short: 'HTTP_PROTOCOL_ERROR', overflow: 'HTTP_PROTOCOL_ERROR' })) {
  cases[`${kind} response cannot create trusted progress`] = () => fixture(async f => {
    const task = f.start(); await rejects(task.wait(), code); assert(task.snapshot().committedBytes === 0n && await f.entry.status() === 'partial');
  }, { kind, options: { limits: { httpRequests: 1 } } });
}

export async function prepareReloadDownload() {
  const namespace = crypto.randomUUID();
  const state = await browserState({ namespace });
  const f = await prepare({ kind: 'slow', options: { state, limits: { httpRequests: 1, commitBytes: BLOCK } } });
  const task = f.start(); await until(task, s => s.verifiedBytes >= BigInt(BLOCK)); await task.pause();
  const result = { namespace, cacheId: f.entry.id, checkpointId: task.snapshot().checkpointId!, seed: Array.from(f.raw) };
  await f.client.close(); await state.close();
  return result;
}
export async function checkReloadDownload(input: Awaited<ReturnType<typeof prepareReloadDownload>>) {
  const state = await browserState({ namespace: input.namespace });
  const cache = await opfsCache(); const entry = await cache.open(input.cacheId);
  const client = createClient({ state });
  try {
    const seed = await client.parseSeed(new Uint8Array(input.seed));
    const task = client.resumeDownload({ seed, destination: entry.destination, checkpointId: input.checkpointId });
    await task.wait(); assert(task.snapshot().receivedBytes === seed.info.size - BigInt(BLOCK));
    assert(await entry.status() === 'complete');
  } finally { await client.close(); await state.close(); await cache.remove(entry.id); }
}
