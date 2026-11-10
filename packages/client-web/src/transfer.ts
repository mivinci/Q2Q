import type {PartialSharing} from './partial-sharing.ts';
import type { PeerSource,PeerBlock } from './peer-download.ts';
import { Q2QError } from './error.ts';
import { abortable, cancelled, httpBytes, normalizeUrls, PermitPool } from './http.ts';
import { seedData } from './seed.ts';
import { createBlockVerifier, createPeerBlockVerifier } from './block-verifier.ts';
import { hasBlock, setBlock } from './checkpoint-codec.ts';
import { resumeCheckpoint } from './resume-checkpoint.ts';
import type { CheckpointSession } from './resume-checkpoint.ts';
import type { ContentVerifier } from './block-verifier.ts';
import type { StateStore } from './checkpoint-types.ts';
import type { DownloadOptions, ClientLimits, SourceProvider, Transfer, DownloadSnapshot, DownloadResult, HttpSources, DownloadState } from './client-types.ts';

function failure(error: unknown): Q2QError { return error instanceof Q2QError ? error : new Q2QError('NETWORK_UNAVAILABLE', true); }

export function createTransfer(options: DownloadOptions & { checkpointId?: string; sharing?:PartialSharing; stopSharing?():Promise<void>; state: StateStore;
  pool: PermitPool; limits: ClientLimits; sourceProvider?: SourceProvider; wasmUrl?: string | URL; openPeers?(committed:Uint8Array,signal:AbortSignal,received:(bytes:number)=>void):Promise<PeerSource>; ended(): void }): Transfer {
  const { seed, limits } = options;
  const data = seedData(seed);
  const id = crypto.randomUUID();
  let checkpointId = options.checkpointId ?? null;
  let mode: 'create' | 'resume' = options.checkpointId ? 'resume' : 'create';
  let desire: 'run' | 'pause' | 'cancel' = 'run';
  let terminal = false;
  let network = new AbortController();
  let preparing: AbortController | undefined;
  let current: Promise<void> = Promise.resolve();
  let commands: Promise<void> = Promise.resolve();
  let received = 0n;
  let verified = 0n;
  let committed = 0n;
  let lastEmission = 0;
  const began = performance.now();
  let snapshot: Readonly<DownloadSnapshot> = Object.freeze({ id, state: 'preparing', content: seed.info,
    receivedBytes: 0n, verifiedBytes: 0n, committedBytes: 0n, bytesPerSecond: 0, checkpointId, error: null });
  const listeners = new Set<(value: Readonly<DownloadSnapshot>) => void>();
  type Outcome = { result: DownloadResult } | { error: Q2QError };
  let settle!: (value: Outcome) => void;
  // The internal completion never rejects: callers need not attach wait immediately.
  const outcome = new Promise<Outcome>(resolve => { settle = resolve; });
  function emit(state: DownloadState = snapshot.state, error: Q2QError | null = null, force = true) {
    const now = performance.now();
    snapshot = Object.freeze({ id, state, content: seed.info, receivedBytes: received, verifiedBytes: verified,
      committedBytes: committed, checkpointId, bytesPerSecond: Number(received) * 1000 / Math.max(1, now - began),
      error: error ? Object.freeze({ code: error.code, retryable: error.retryable, ...(error.diagnostic ? { diagnostic: error.diagnostic } : {}) }) : null });
    if (!force && now - lastEmission < 100) return;
    lastEmission = now;
    for (const listener of listeners) {
      try { void Promise.resolve(listener(snapshot)).catch(() => {}); } catch { /* observers cannot stop IO */ }
    }
  }
  function end(result: Outcome) {
    if (terminal) return;
    terminal = true;
    if ('result' in result) emit('completed');
    else emit(result.error.code === 'TASK_CANCELLED' ? 'cancelled' : 'failed', result.error);
    listeners.clear(); options.ended(); settle(result);
  }
  function check() { if (desire !== 'run' || network.signal.aborted) throw cancelled(); }
  async function run() {
    network = new AbortController();
    preparing = new AbortController();
    const prep = preparing;
    let session: CheckpointSession | undefined;
    let verifier: ContentVerifier | undefined;
    let dirty = 0;
    let peerSource:PeerSource|undefined;let peerOpening:Promise<PeerSource>|undefined;
    let problem: Q2QError | undefined;
    let success:DownloadResult|undefined;
    try {
      check(); emit('preparing');
      let refreshed = false;
      let refreshPromise: Promise<void> | undefined;
      let sourceRevision = 0;
      let sources: HttpSources;
      async function resolve(reason: 'initial' | 'unauthorized') {
        const value = options.sourceProvider
          ? await abortable(Promise.resolve().then(() => options.sourceProvider!.resolve({ content: seed.info, reason, signal: network.signal })), network.signal)
          : { contentUrls: data.contentUrls, proofUrls: data.proofUrls };
        sourceRevision++;
        sources = { contentUrls: normalizeUrls(value.contentUrls), proofUrls: normalizeUrls(value.proofUrls ?? []) };
      }
      await resolve('initial');
      async function obtain(kind: 'content' | 'proof', length: number, range?: { start: bigint; end: bigint; total: bigint }): Promise<Uint8Array> {
        let last = new Q2QError(kind === 'proof' ? 'PROOF_UNAVAILABLE' : 'NO_SOURCES');
        for (;;) {
          const revision = sourceRevision;
          const urls = kind === 'proof' ? sources.proofUrls ?? [] : sources.contentUrls;
          let refresh = false;
          for (const url of urls) {
            for (let attempt = 0; attempt < limits.attemptsPerSource; attempt++) {
              check();
              if (snapshot.state !== 'preparing') emit('downloading');
              try {
                const bytes = await httpBytes({ pool: options.pool, url, length, range, signal: network.signal,
                  timeoutMs: limits.requestTimeoutMs, received: kind === 'content' ? count => { received += BigInt(count); emit(undefined, null, false); } : undefined });
                // Content corruption is a source failure, so another origin may repair it.
                if (kind === 'content' && verifier && !verifier.verify(range!.start / BigInt(seed.info.blockSize), bytes)) throw new Q2QError('CONTENT_MISMATCH');
                if (kind === 'proof') {
                  try { verifier = await createBlockVerifier({ seed: data.bytes, proof: bytes, wasmUrl: options.wasmUrl }); }
                  catch (error) { if (error instanceof Q2QError && error.code === 'INVALID_SEED') throw new Q2QError('CONTENT_MISMATCH'); throw error; }
                }
                return bytes;
              } catch (error) {
                check(); last = failure(error);
                if (last.code === 'AUTH_REQUIRED' && options.sourceProvider && (!refreshed || refreshPromise || revision !== sourceRevision)) { refresh = true; break; }
                if (!last.retryable || attempt + 1 === limits.attemptsPerSource) break;
                if (snapshot.state !== 'preparing') emit('waiting');
                await new Promise<void>((resolve, reject) => {
                  const stop = () => { clearTimeout(timer); reject(cancelled()); };
                  const timer = setTimeout(() => { network.signal.removeEventListener('abort', stop); resolve(); }, limits.retryDelayMs);
                  network.signal.addEventListener('abort', stop, { once: true });
                  if (network.signal.aborted) stop();
                });
              }
            }
            if (refresh) break;
          }
          if (!refresh) throw last;
          if (revision !== sourceRevision) continue;
          if (!refreshed) {
            refreshed = true;
            refreshPromise = resolve('unauthorized').finally(() => { refreshPromise = undefined; });
          }
          if (refreshPromise) await refreshPromise;
          else throw last;
        }
      }
      if (data.inline || seed.info.blockCount === 0n) verifier = await createBlockVerifier({ seed: data.bytes, wasmUrl: options.wasmUrl });
      else {
        try{await obtain('proof',Number(seed.info.blockCount)*32);}
        catch(error){check();if(!options.openPeers)throw error;verifier=await createPeerBlockVerifier({seed:data.bytes,wasmUrl:options.wasmUrl});}
      }
      check();
      if (checkpointId === null) {
        const record = await options.state.create({ content: seed.info, targetId: options.destination.targetId });
        checkpointId = record.id; emit('preparing');
      }
      session = await resumeCheckpoint({ state: options.state, checkpointId, verifier: verifier!, destination: options.destination,
        signal: prep.signal, mode, maxPendingBytes: Math.max(seed.info.blockSize, limits.commitBytes) });
      // Keep this signal alive through commit/abort. Network cancellation alone must
      // not discard already verified blocks before their checkpoint is committed.
      preparing = undefined; mode = 'resume';
      options.sharing?.attach(session);
      const bitmap = session.snapshot().committed;
      verified = committed = session.snapshot().committedBytes;
      check(); emit('downloading');
      let next = 0;
      let io: Promise<void> = Promise.resolve();
      let workerError: Q2QError | undefined;
      async function worker() {
        try {
          for (;;) {
            check();
            while (next < verifier!.blockCount && hasBlock(bitmap, next)) next++;
            if (next >= verifier!.blockCount) return;
            const index = next++;
            const start = BigInt(index) * BigInt(seed.info.blockSize);
            const length = Number(seed.info.size - start < BigInt(seed.info.blockSize) ? seed.info.size - start : BigInt(seed.info.blockSize));
            let peerBlock:PeerBlock|undefined;let peerError:Q2QError|undefined;
            if(options.openPeers){
              try{
                peerOpening??=options.openPeers(bitmap,network.signal,count=>{received+=BigInt(count);emit(undefined,null,false);}).then(value=>{peerSource=value;return value;});
                const source=await peerOpening;peerBlock=await source.fetch(index,length,(bytes,proof)=>verifier!.verifyPeer?verifier!.verifyPeer(BigInt(index),bytes,proof):verifier!.verify(BigInt(index),bytes));
              }catch(error){check();peerError=failure(error);}
            }
            let bytes:Uint8Array;
            if(peerBlock)bytes=peerBlock.bytes;
            else if(verifier!.requiresWitness)throw peerError??new Q2QError('PROOF_UNAVAILABLE');
            else if(sources.contentUrls.length===0&&peerError)throw peerError;
            else bytes = await obtain('content', length, { start, end: start + BigInt(length) - 1n, total: seed.info.size });
            const write = io.then(async () => {
              check(); await session!.writeBlock(BigInt(index), bytes,peerBlock?.witness);
              setBlock(bitmap, index); verified += BigInt(length); dirty += length;
              if (peerBlock || dirty >= limits.commitBytes) { committed = (await session!.commit()).committedBytes; options.sharing?.committed();dirty = 0; }
              if(peerBlock)await peerBlock.acknowledge();
              emit(undefined, null, false);
            });
            io = write.then(() => undefined, () => undefined);
            await write;
          }
        } catch (error) {
          if (!workerError) { workerError = failure(error); network.abort(); }
        }
      }
      await Promise.all(Array.from({ length: Math.min(limits.httpRequests, Math.max(1, verifier!.blockCount)) }, worker));
      await io;
      if (workerError) throw workerError;
      check(); emit('finalizing');
      options.sharing?.end();await options.stopSharing?.();
      const output = await session.finish(); committed = verified = seed.info.size; dirty=0;
      check(); success=Object.freeze({ content: seed.info, output });
    } catch (error) { problem = failure(error); }
    finally {
      preparing = undefined;
      options.sharing?.end();
      try{await options.stopSharing?.();}catch(error){problem=failure(error);}
      if (session) {
        try {
          if (!terminal && dirty > 0) { committed = (await session.commit()).committedBytes; dirty = 0; }
        } catch (error) { problem = failure(error); }
        try { await session.abort(); } catch (error) { problem = failure(error); }
      }
      try{await peerSource?.close();}catch(error){problem=failure(error);}
      finally{verifier?.close();}
    }
    if (terminal) return;
    // Completion owns transport cleanup too; wait()/Client.close() must not race
    // a still-live peer after the destination has been finalized.
    if(success&&!problem){if(desire==='run'){end({result:success});return;}problem=cancelled();}
    if (desire === 'pause' && problem?.code === 'TASK_CANCELLED') emit('paused');
    else if (desire === 'cancel' && (!problem || problem.code === 'TASK_CANCELLED')) end({ error: cancelled() });
    else end({ error: problem ?? cancelled() });
  }
  function launch() { current = Promise.resolve().then(run).catch(error => end({ error: failure(error) })); }
  function command(operation: () => Promise<void>) {
    const result = commands.then(operation); commands = result.catch(() => {}); return result;
  }
  const task: Transfer = Object.freeze({
    id, snapshot: () => snapshot,
    subscribe(listener: (value: Readonly<DownloadSnapshot>) => void) { if (!terminal) listeners.add(listener); try { void Promise.resolve(listener(snapshot)).catch(() => {}); } catch { /* isolated */ } return () => { listeners.delete(listener); }; },
    pause() {
      if (terminal) return Promise.reject(new Q2QError('INVALID_STATE'));
      if (snapshot.state === 'paused') return Promise.resolve();
      if (desire !== 'cancel') { desire = 'pause'; network.abort(); preparing?.abort(); emit('pausing'); }
      return command(async () => { await current; if (snapshot.state === 'failed') throw new Q2QError(snapshot.error!.code, snapshot.error!.retryable, snapshot.error!.diagnostic); });
    },
    resume() {
      return command(async () => {
        if (!terminal && desire === 'run') return;
        await current;
        if (terminal || desire === 'cancel') throw new Q2QError('INVALID_STATE');
        if (snapshot.state !== 'paused') return;
        desire = 'run'; launch();
      });
    },
    cancel() {
      if (terminal) return Promise.resolve();
      desire = 'cancel'; network.abort(); preparing?.abort(); emit('cancelling');
      return command(async () => {
        await current;
        if (!terminal) end({ error: cancelled() });
        if (snapshot.state === 'failed') throw new Q2QError(snapshot.error!.code, snapshot.error!.retryable, snapshot.error!.diagnostic);
      });
    },
    async wait() { const value = await outcome; if ('error' in value) throw value.error; return value.result; }
  });
  launch(); return task;
}
