import { Q2QError } from './error.ts';
import { cancelled } from './http.ts';
import type { PermitPool } from './http.ts';
import { sourcePipeline } from './source.ts';
import { resumeCheckpoint } from './resume-checkpoint.ts';
import type { CheckpointSession } from './resume-checkpoint.ts';
import { hasBlock } from './checkpoint-codec.ts';
import type { StateStore } from './checkpoint-types.ts';
import type { PeerSource } from './peer-download.ts';
import type { ClientLimits, ContentInfo, DownloadSnapshot, Seed, SourceProvider } from './client-types.ts';
import type { ContentDestination } from './storage.ts';

export interface StreamOptions {
  seed: Seed;
  /** Whole-file target; verified blocks are written here and survive close(). */
  destination: ContentDestination;
  /** Blocks fetched ahead of the cursor (default 8, maximum 64; 0 fetches only the current block). */
  prefetchBlocks?: number;
  /** Maximum wait for one missing block, including verification and disk IO. */
  readTimeoutMs?: number;
}

export interface StreamReader {
  readonly info: ContentInfo;
  readonly position: bigint;
  readonly checkpointId: string;
  /** Waits for the block under the cursor, then returns verified bytes (a read
   * never crosses a block boundary and may return fewer than `length` bytes). */
  read(length: number, options?: { signal?: AbortSignal }): Promise<Uint8Array>;
  seek(offset: bigint): Promise<void>;
  progress(): Readonly<DownloadSnapshot>;
  /** Cancels network work; verified blocks and the checkpoint stay resumable. */
  close(): Promise<void>;
}

const DEFAULT_PREFETCH = 8;
const MAX_PREFETCH = 64;
const DEFAULT_READ_TIMEOUT = 30_000;
const MAX_READ_TIMEOUT = 600_000;

type Waiter = { resolve(bytes: Uint8Array): void; reject(error: Q2QError): void };

/** One verified cursor over a bounded, seek-driven window. Sparse bytes are
 * never returned: reads are served by readCommittedBlock after this session
 * wrote and committed the block. Mirrors the native reader semantics. */
export function createStream(options: {
  seed: Seed; destination: ContentDestination; state: StateStore; checkpointId?: string;
  pool: PermitPool; limits: ClientLimits; sourceProvider?: SourceProvider; wasmUrl?: string | URL;
  openPeers?(committed: Uint8Array, signal: AbortSignal, received: (bytes: number) => void): Promise<PeerSource>;
  prefetchBlocks?: number; readTimeoutMs?: number;
}): StreamReader {
  const { seed, limits } = options;
  const prefetch = options.prefetchBlocks ?? DEFAULT_PREFETCH;
  const readTimeout = options.readTimeoutMs ?? DEFAULT_READ_TIMEOUT;
  if (!Number.isSafeInteger(prefetch) || prefetch < 0 || prefetch > MAX_PREFETCH) throw new Q2QError('INVALID_ARGUMENT');
  if (!Number.isSafeInteger(readTimeout) || readTimeout <= 0 || readTimeout > MAX_READ_TIMEOUT) throw new Q2QError('INVALID_ARGUMENT');
  const blockSize = BigInt(seed.info.blockSize);
  const total = Number(seed.info.blockCount);
  const controller = new AbortController();
  let checkpointId = options.checkpointId ?? '';
  let session: CheckpointSession | undefined;
  let pipeline: ReturnType<typeof sourcePipeline> | undefined;
  let peerSource: PeerSource | undefined;
  let peerOpening: Promise<PeerSource> | undefined;
  let position = 0n;
  let received = 0n, verified = 0n, committed = 0n;
  const began = performance.now();
  let state: DownloadSnapshot['state'] = 'preparing';
  let failure: Q2QError | undefined;
  let closed = false;
  let closing: Promise<void> | undefined;
  let chain: Promise<unknown> = Promise.resolve();
  /** Current window is [windowStart, windowEnd]; indices outside are withdrawn. */
  let windowStart = -1, windowEnd = -1;
  const inFlight = new Map<number, Promise<void>>();
  const waiters = new Map<number, Set<Waiter>>();
  const concurrency = Math.max(1, Math.min(prefetch + 1, limits.peerConnections + limits.httpRequests));

  function snapshot(): Readonly<DownloadSnapshot> {
    return Object.freeze({ id: `sha256:${seed.info.id.slice(7)}`, state, content: seed.info, receivedBytes: received,
      verifiedBytes: verified, committedBytes: committed,
      bytesPerSecond: Number(received) * 1000 / Math.max(1, performance.now() - began),
      checkpointId: checkpointId || null,
      error: failure ? Object.freeze({ code: failure.code, retryable: failure.retryable,
        ...(failure.diagnostic ? { diagnostic: failure.diagnostic } : {}) }) : null });
  }
  function check() {
    if (closed || controller.signal.aborted) throw new Q2QError('CLIENT_CLOSED');
    if (failure) throw failure;
  }
  function settle(index: number, bytes: Uint8Array) {
    const set = waiters.get(index); waiters.delete(index);
    for (const waiter of set ?? []) waiter.resolve(bytes);
  }
  function fail(index: number, error: Q2QError) {
    const set = waiters.get(index); waiters.delete(index);
    for (const waiter of set ?? []) waiter.reject(error);
  }

  async function setup() {
    pipeline = sourcePipeline({ seed, signal: controller.signal, pool: options.pool, limits,
      sourceProvider: options.sourceProvider, wasmUrl: options.wasmUrl,
      onReceived: bytes => { received += BigInt(bytes); } });
    await pipeline.prepareSources();
    // Windowed reads verify against the proof table; peer-only content has no
    // proof frame to read back, so it reports unsupported instead of serving
    // bytes that cannot be re-verified on read.
    const verifier = await pipeline.prepareVerifier({ peerOnly: false });
    if (options.checkpointId) {
      checkpointId = options.checkpointId;
    } else {
      const record = await options.state.create({ content: seed.info, targetId: options.destination.targetId });
      checkpointId = record.id;
    }
    session = await resumeCheckpoint({ state: options.state, checkpointId, verifier, destination: options.destination,
      signal: controller.signal, mode: options.checkpointId ? 'resume' : 'create',
      maxPendingBytes: Math.max(seed.info.blockSize, limits.commitBytes) });
    committed = verified = session.snapshot().committedBytes;
    state = 'downloading';
  }

  async function readBlock(index: number): Promise<Uint8Array> {
    const block = await session!.readCommittedBlock(BigInt(index));
    if (!block) throw new Q2QError('NO_SOURCES');
    return block.bytes;
  }

  /** Peers first, then HTTP; the block is committed before it can be read. */
  async function fetchBlock(index: number) {
    try {
      check();
      const start = BigInt(index) * blockSize;
      const remaining = seed.info.size - start;
      const length = Number(remaining < blockSize ? remaining : blockSize);
      let written = false;
      if (options.openPeers) {
        try {
          peerOpening ??= options.openPeers(session!.snapshot().committed, controller.signal, bytes => { received += BigInt(bytes); });
          peerSource = await peerOpening;
          const block = await peerSource.fetch(index, length, (bytes, proof) => pipeline!.verifier().verifyPeer
            ? pipeline!.verifier().verifyPeer!(BigInt(index), bytes, proof)
            : pipeline!.verifier().verify(BigInt(index), bytes));
          await session!.writeBlock(BigInt(index), block.bytes, block.witness);
          verified += BigInt(length);
          committed = (await session!.commit()).committedBytes;
          await block.acknowledge();
          written = true;
        } catch (error) {
          check();
          // A peer failure only falls back when another source can serve it.
          if (pipeline!.contentSourceCount() === 0) throw error instanceof Q2QError ? error : new Q2QError('PEER_CLOSED');
        }
      }
      if (!written) {
        const bytes = await pipeline!.obtain('content', length, { start, end: start + BigInt(length) - 1n, total: seed.info.size });
        await session!.writeBlock(BigInt(index), bytes);
        verified += BigInt(length);
        committed = (await session!.commit()).committedBytes;
      }
      settle(index, await readBlock(index));
    } catch (error) {
      // One failed block fails only the reads waiting on it; the window stays usable.
      fail(index, error instanceof Q2QError ? error : new Q2QError('NETWORK_UNAVAILABLE', true));
    } finally {
      inFlight.delete(index);
      pump();
    }
  }

  /** Starts queued window blocks up to the concurrency budget. */
  function pump() {
    if (closed || failure || !session) return;
    for (let index = windowStart; index <= windowEnd && inFlight.size < concurrency; index++) {
      if (index < 0 || index >= total) continue;
      if (inFlight.has(index) || hasBlock(session.snapshot().committed, index)) continue;
      inFlight.set(index, fetchBlock(index));
    }
  }

  /** Focus moves the window; queued work outside it is dropped, in-flight work
   * still completes and remains cached for a later seek. */
  function focus(index: number) {
    const start = Math.min(Math.max(0, index), Math.max(0, total - 1));
    const end = Math.min(total - 1, start + prefetch);
    if (start === windowStart && end === windowEnd) { pump(); return; }
    windowStart = start; windowEnd = end;
    pump();
  }

  async function awaitBlock(index: number): Promise<Uint8Array> {
    if (hasBlock(session!.snapshot().committed, index)) return readBlock(index);
    return new Promise<Uint8Array>((resolve, reject) => {
      const set = waiters.get(index) ?? new Set<Waiter>();
      waiters.set(index, set);
      const timer = setTimeout(() => { cleanup(); reject(new Q2QError('CONTROL_TIMEOUT', true)); }, readTimeout);
      const abort = () => { cleanup(); reject(cancelled()); };
      const waiter: Waiter = {
        resolve(bytes) { cleanup(); resolve(bytes); },
        reject(error) { cleanup(); reject(error); }
      };
      function cleanup() {
        clearTimeout(timer);
        controller.signal.removeEventListener('abort', abort);
        set.delete(waiter);
        if (!set.size) waiters.delete(index);
      }
      controller.signal.addEventListener('abort', abort, { once: true });
      set.add(waiter);
      focus(index);
    });
  }

  const opening = setup().catch(error => {
    failure = error instanceof Q2QError ? error : new Q2QError('STORAGE_UNAVAILABLE', true);
    state = 'failed';
    throw failure;
  });
  void opening.catch(() => {});

  const reader: StreamReader & { ready(): Promise<void> } = Object.freeze({
    info: seed.info,
    get position() { return position; },
    get checkpointId() { return checkpointId; },
    ready: () => opening,
    progress: snapshot,
    async seek(offset: bigint) {
      if (typeof offset !== 'bigint' || offset < 0n || offset > seed.info.size) throw new Q2QError('INVALID_RANGE');
      await opening;
      check();
      position = offset;
      focus(position >= seed.info.size ? Math.max(0, total - 1) : Number(position / blockSize));
    },
    read(length: number, settings: { signal?: AbortSignal } = {}) {
      const run = async () => {
        await opening;
        check();
        if (settings.signal?.aborted) throw cancelled();
        if (!Number.isSafeInteger(length) || length <= 0) throw new Q2QError('INVALID_RANGE');
        if (position >= seed.info.size) return new Uint8Array();
        const index = Number(position / blockSize);
        const offset = Number(position % blockSize);
        const block = await awaitBlock(index);
        const available = block.length - offset;
        const amount = Math.min(length, available, Number(seed.info.size - position));
        position += BigInt(amount);
        return block.slice(offset, offset + amount);
      };
      const work = chain.then(run);
      chain = work.catch(() => {});
      return work;
    },
    close() {
      if (closing) return closing;
      closed = true;
      controller.abort();
      for (const index of [...waiters.keys()]) fail(index, new Q2QError('TASK_CANCELLED'));
      waiters.clear();
      closing = (async () => {
        await opening.catch(() => {});
        // Network work is already aborted; wait for the fetches to unwind before
        // releasing the session so no write lands after abort().
        await Promise.allSettled([...inFlight.values()]);
        try { await peerSource?.close(); } catch { /* best effort */ }
        try { await session?.abort(); } catch { /* verified blocks stay resumable */ }
        pipeline?.close();
      })();
      return closing;
    }
  });
  return reader;
}
