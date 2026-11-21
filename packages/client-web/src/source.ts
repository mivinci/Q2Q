import { Q2QError } from './error.ts';
import { abortable, httpBytes, normalizeUrls } from './http.ts';
import type { PermitPool } from './http.ts';
import { createBlockVerifier, createPeerBlockVerifier } from './block-verifier.ts';
import type { ContentVerifier } from './block-verifier.ts';
import { seedData } from './seed.ts';
import type { ClientLimits, HttpSources, Seed, SourceProvider } from './client-types.ts';

/** Shared source resolution: credential-free URLs, retries, one auth refresh and
 * content verification. Both whole-file downloads and windowed reads use it so
 * the two paths cannot drift on authorization or corruption handling. */
export interface SourcePipeline {
  verifier(): ContentVerifier;
  /** Resolves initial source URLs once; later calls reuse the result. */
  prepareSources(): Promise<void>;
  /** HTTP content URL count after `prepareSources`; peers may exist without any. */
  contentSourceCount(): number;
  /** Loads the proof table; with `peerOnly` it may fall back to a witness verifier. */
  prepareVerifier(options: { peerOnly: boolean }): Promise<ContentVerifier>;
  obtain(kind: 'content' | 'proof', length: number, range?: { start: bigint; end: bigint; total: bigint }): Promise<Uint8Array>;
  close(): void;
}

export function sourcePipeline(options: {
  seed: Seed; signal: AbortSignal; pool: PermitPool; limits: ClientLimits;
  sourceProvider?: SourceProvider; wasmUrl?: string | URL;
  onReceived?(bytes: number): void;
  /** Notified before each HTTP attempt that is about to change visible state. */
  onAttempt?(): void;
  onWaiting?(): void;
}): SourcePipeline {
  const data = seedData(options.seed);
  const limits = options.limits;
  let verifier: ContentVerifier | undefined;
  let sources: HttpSources = { contentUrls: [], proofUrls: [] };
  let refreshed = false;
  let refreshPromise: Promise<void> | undefined;
  let revision = 0;
  let initial: Promise<void> | undefined;

  async function resolve(reason: 'initial' | 'unauthorized') {
    const value = options.sourceProvider
      ? await abortable(Promise.resolve().then(() => options.sourceProvider!.resolve({ content: options.seed.info, reason, signal: options.signal })), options.signal)
      : { contentUrls: data.contentUrls, proofUrls: data.proofUrls };
    revision++;
    sources = { contentUrls: normalizeUrls(value.contentUrls), proofUrls: normalizeUrls(value.proofUrls ?? []) };
  }
  function check() { if (options.signal.aborted) throw new Q2QError('TASK_CANCELLED'); }

  async function obtain(kind: 'content' | 'proof', length: number, range?: { start: bigint; end: bigint; total: bigint }): Promise<Uint8Array> {
    initial ??= resolve('initial');
    await initial;
    let last = new Q2QError(kind === 'proof' ? 'PROOF_UNAVAILABLE' : 'NO_SOURCES');
    for (;;) {
      const seen = revision;
      const urls = kind === 'proof' ? sources.proofUrls ?? [] : sources.contentUrls;
      let refresh = false;
      for (const url of urls) {
        for (let attempt = 0; attempt < limits.attemptsPerSource; attempt++) {
          check();
          options.onAttempt?.();
          try {
            const bytes = await httpBytes({ pool: options.pool, url, length, range, signal: options.signal,
              timeoutMs: limits.requestTimeoutMs, received: kind === 'content' ? bytes => options.onReceived?.(bytes) : undefined });
            // Content corruption is a source failure, so another origin may repair it.
            if (kind === 'content' && verifier && !verifier.verify(range!.start / BigInt(options.seed.info.blockSize), bytes)) throw new Q2QError('CONTENT_MISMATCH');
            if (kind === 'proof') {
              try { verifier = await createBlockVerifier({ seed: data.bytes, proof: bytes, wasmUrl: options.wasmUrl }); }
              catch (error) { if (error instanceof Q2QError && error.code === 'INVALID_SEED') throw new Q2QError('CONTENT_MISMATCH'); throw error; }
            }
            return bytes;
          } catch (error) {
            check(); last = failure(error);
            if (last.code === 'AUTH_REQUIRED' && options.sourceProvider && (!refreshed || refreshPromise || seen !== revision)) { refresh = true; break; }
            if (!last.retryable || attempt + 1 === limits.attemptsPerSource) break;
            options.onWaiting?.();
            await new Promise<void>((resolve, reject) => {
              const stop = () => { clearTimeout(timer); reject(new Q2QError('TASK_CANCELLED')); };
              const timer = setTimeout(() => { options.signal.removeEventListener('abort', stop); resolve(); }, limits.retryDelayMs);
              options.signal.addEventListener('abort', stop, { once: true });
              if (options.signal.aborted) stop();
            });
          }
        }
        if (refresh) break;
      }
      if (!refresh) throw last;
      if (seen !== revision) continue;
      if (!refreshed) {
        refreshed = true;
        refreshPromise = resolve('unauthorized').finally(() => { refreshPromise = undefined; });
      }
      if (refreshPromise) await refreshPromise;
      else throw last;
    }
  }

  return Object.freeze({
    verifier() {
      if (!verifier) throw new Q2QError('INVALID_STATE');
      return verifier;
    },
    async prepareSources() { initial ??= resolve('initial'); await initial; },
    contentSourceCount() { return sources.contentUrls.length; },
    async prepareVerifier(settings: { peerOnly: boolean }) {
      await (initial ??= resolve('initial'));
      if (data.inline || options.seed.info.blockCount === 0n) {
        verifier = await createBlockVerifier({ seed: data.bytes, wasmUrl: options.wasmUrl });
        return verifier;
      }
      try { await obtain('proof', Number(options.seed.info.blockCount) * 32); }
      catch (error) {
        check();
        if (!settings.peerOnly) throw error;
        verifier = await createPeerBlockVerifier({ seed: data.bytes, wasmUrl: options.wasmUrl });
      }
      return verifier!;
    },
    obtain,
    close() { verifier?.close(); verifier = undefined; }
  });
}

function failure(error: unknown): Q2QError {
  return error instanceof Q2QError ? error : new Q2QError('NETWORK_UNAVAILABLE', true);
}
