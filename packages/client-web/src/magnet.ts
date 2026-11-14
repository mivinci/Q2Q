import { Q2QError } from './error.ts';
import { abortable } from './http.ts';
import { PermitPool } from './http.ts';
import { initializeWasm } from './block-verifier.ts';
import { parseSeed } from './seed.ts';
import { magnetCache } from './magnet-cache.ts';
import { peerNetwork } from './network.ts';
import type { NetworkSession } from './network.ts';
import type { PeerConnection, PeerCandidate } from './peer-dial.ts';
import type { MagnetLink, Seed } from './client-types.ts';

/** Core parity: link 16 KiB, raw info 8 KiB; base64 grows by 4/3. */
const MAX_MAGNET_CHARS = 16 * 1024;
const MAX_INFO_B64 = Math.ceil((8 * 1024) / 3) * 4;
const RESOLVE_BUDGET_MS = 60_000;
const INFO_WAIT_MS = 10_000;
const DIAL_CONCURRENCY = 2;

const REQUEST_INFO = '{"v":1,"message":{"type":"request_info"}}';

/** Pure local parse via the shared Rust implementation; no network access. */
export async function parseMagnetLink(link: string, signal: AbortSignal, wasmUrl?: string | URL): Promise<MagnetLink> {
  if (typeof link !== 'string' || link.length === 0 || link.length > MAX_MAGNET_CHARS) throw new Q2QError('INVALID_MAGNET');
  const module = await abortable(initializeWasm(wasmUrl), signal);
  if (signal.aborted) throw new Q2QError('TASK_CANCELLED');
  let parsed: { infoHash?: unknown; displayName?: unknown; trackers?: unknown };
  try {
    parsed = JSON.parse(module.parse_magnet(link)) as typeof parsed;
  } catch (error) {
    // wasm throws stable machine codes; only the namespace split matters to callers.
    if (error instanceof Error && error.message === 'wrong_namespace') throw new Q2QError('MAGNET_UNSUPPORTED');
    throw new Q2QError('INVALID_MAGNET');
  }
  if (typeof parsed.infoHash !== 'string' || !/^[a-f0-9]{64}$/.test(parsed.infoHash)
    || !(parsed.displayName === null || typeof parsed.displayName === 'string')
    || !Array.isArray(parsed.trackers) || parsed.trackers.some(item => typeof item !== 'string')) {
    throw new Q2QError('INVALID_MAGNET');
  }
  return Object.freeze({
    infoHash: parsed.infoHash,
    displayName: parsed.displayName,
    trackers: Object.freeze([...parsed.trackers as string[]]),
  });
}

/** Cache hit requires re-verification, never blind trust in stored bytes. */
async function cachedMagnetSeed(link: MagnetLink, signal: AbortSignal, wasmUrl?: string | URL): Promise<Seed | null> {
  const cache = await magnetCache();
  if (!cache) return null;
  const id = `sha256:${link.infoHash}`;
  let bytes: Uint8Array | null = null;
  try { bytes = await cache.get(id); } catch { return null; }
  if (!bytes) return null;
  try {
    const seed = await parseSeed(bytes, signal, wasmUrl);
    if (seed.info.id === id) return seed;
  } catch (error) {
    if (signal.aborted) throw error;
  }
  // Corrupt or mismatched records stay a miss; a later success overwrites them.
  return null;
}

export interface ResolveMagnetOptions {
  readonly link: MagnetLink;
  connect(): Promise<NetworkSession>;
  readonly permits: PermitPool;
  readonly signal: AbortSignal;
  readonly wasmUrl?: string | URL;
}

/** Bounded metadata discovery from online peers; no destination is opened here. */
export async function resolveMagnet(options: ResolveMagnetOptions): Promise<Seed> {
  const link = options.link;
  const id = `sha256:${link.infoHash}`;
  const cached = await cachedMagnetSeed(link, options.signal, options.wasmUrl);
  if (cached) return cached;
  const wasm = await abortable(initializeWasm(options.wasmUrl), options.signal);
  const hash = new Uint8Array(32);
  for (let index = 0; index < 32; index++) hash[index] = parseInt(link.infoHash.slice(index * 2, index * 2 + 2), 16);
  const budget = AbortSignal.timeout(RESOLVE_BUDGET_MS);
  const deadline = AbortSignal.any([options.signal, budget]);
  // Success cancels remaining candidates; each attempt owns its connection.
  const settled = new AbortController();
  const attempt = AbortSignal.any([deadline, settled.signal]);
  const session = await abortable(options.connect(), deadline);
  const network = peerNetwork(session);
  // Track's cross-object file index converges after publication announce.
  let candidates = await network.query(id, deadline);
  for (let retry = 0; candidates.length === 0 && retry < 3; retry++) {
    await new Promise<void>((resolve, reject) => {
      const stop = () => { clearTimeout(timer); reject(new Q2QError('TASK_CANCELLED')); };
      const timer = setTimeout(() => { deadline.removeEventListener('abort', stop); resolve(); }, 250);
      deadline.addEventListener('abort', stop, { once: true });
      if (deadline.aborted) stop();
    });
    candidates = await network.query(id, deadline);
  }
  let cursor = 0;
  let tried = 0;
  let metadataFailed = false;
  let last: Q2QError | undefined;
  let winner: Seed | undefined;

  async function attemptResolve(candidate: PeerCandidate): Promise<Seed> {
    const release = await options.permits.acquire(attempt);
    let connection: PeerConnection | undefined;
    try {
      connection = await network.dial(candidate, hash, attempt);
      await connection.rtc.sendControl(REQUEST_INFO, attempt);
      const infoWait = AbortSignal.timeout(INFO_WAIT_MS);
      let raw: string;
      try {
        raw = await connection.rtc.readControl(AbortSignal.any([attempt, infoWait]));
      } catch (error) {
        if (infoWait.aborted && !attempt.aborted) throw connection.rtc.diagnosed(new Q2QError('CONTROL_TIMEOUT', true));
        throw error;
      }
      let message: { type?: unknown; data?: unknown };
      try { message = JSON.parse(wasm.decode_peer_control(raw)) as typeof message; }
      catch { throw new Q2QError('PEER_PROTOCOL_ERROR'); }
      if (message.type !== 'info' || typeof message.data !== 'string') throw new Q2QError('PEER_PROTOCOL_ERROR');
      if (message.data.length > MAX_INFO_B64) throw new Q2QError('METADATA_MISMATCH');
      let bytes: Uint8Array;
      try { bytes = wasm.magnet_seed(message.data, hash); }
      catch { throw new Q2QError('METADATA_MISMATCH'); }
      const seed = await parseSeed(bytes, attempt, options.wasmUrl);
      if (seed.info.id !== id) throw new Q2QError('METADATA_MISMATCH');
      return seed;
    } finally {
      await connection?.close().catch(() => {});
      release();
    }
  }

  async function worker() {
    for (;;) {
      if (winner) return;
      const candidate = candidates[cursor++];
      if (!candidate) return;
      tried++;
      try {
        const seed = await attemptResolve(candidate);
        if (!winner) {
          winner = seed;
          settled.abort();
        }
      } catch (error) {
        if (winner || options.signal.aborted) return;
        const failure = error instanceof Q2QError ? error : new Q2QError('NETWORK_UNAVAILABLE', true);
        if (failure.code === 'METADATA_MISMATCH') metadataFailed = true;
        last = failure;
      }
    }
  }
  await Promise.all(Array.from({ length: Math.min(DIAL_CONCURRENCY, candidates.length) }, worker));
  if (winner) {
    // Caching is an optimization: quota failures degrade to rediscovery, never
    // fail a verified transfer. Bytes are re-verified on every read anyway.
    const bytes = winner.toBytes();
    try { (await magnetCache())?.put(id, bytes).catch(() => {}); } catch { /* unavailable */ }
    return winner;
  }
  if (options.signal.aborted) throw new Q2QError('TASK_CANCELLED');
  if (budget.aborted) throw new Q2QError('CONTROL_TIMEOUT', true);
  // tried === 0 means the Track listed no one; distinguish it from candidates
  // that existed but failed, where the last connection error is the honest cause.
  if (tried === 0) throw new Q2QError('NO_SOURCES');
  if (metadataFailed) throw new Q2QError('METADATA_MISMATCH');
  if (last) throw last;
  throw new Q2QError('NO_SOURCES');
}

export { cachedMagnetSeed };
