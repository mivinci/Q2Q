import { Q2QError } from './error.ts';

export function cancelled() { return new Q2QError('TASK_CANCELLED'); }
export function abortable<T>(promise: Promise<T>, signal: AbortSignal): Promise<T> {
  return new Promise((resolve, reject) => {
    const abort = () => reject(cancelled());
    if (signal.aborted) { void promise.catch(() => {}); reject(cancelled()); return; }
    signal.addEventListener('abort', abort, { once: true });
    void promise.then(value => { signal.removeEventListener('abort', abort); resolve(value); }, error => {
      signal.removeEventListener('abort', abort); reject(error);
    });
  });
}

export class PermitPool {
  private active = 0;
  private waiting: Array<{ signal: AbortSignal; acquire(): void; abort(): void }> = [];
  private readonly limit: number;
  constructor(limit: number) { this.limit = limit; }
  acquire(signal: AbortSignal): Promise<() => void> {
    return new Promise((resolve, reject) => {
      if (signal.aborted) { reject(cancelled()); return; }
      const waiter = {
        signal,
        acquire: () => {
          signal.removeEventListener('abort', waiter.abort);
          this.active++;
          let released = false;
          resolve(() => {
            if (released) return;
            released = true; this.active--;
            this.waiting.shift()?.acquire();
          });
        },
        abort: () => {
          this.waiting = this.waiting.filter(item => item !== waiter);
          reject(cancelled());
        }
      };
      if (this.active < this.limit) waiter.acquire();
      else { this.waiting.push(waiter); signal.addEventListener('abort', waiter.abort, { once: true }); }
    });
  }
}
export function normalizeUrls(urls: readonly string[]): readonly string[] {
  if (!Array.isArray(urls) || urls.length > 8) throw new Q2QError('INVALID_SOURCE');
  return Object.freeze([...new Set(urls.map(value => {
    if (typeof value !== 'string' || value.length > 8192) throw new Q2QError('INVALID_SOURCE');
    let url: URL;
    try { url = new URL(value); } catch { throw new Q2QError('INVALID_SOURCE'); }
    if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.hash) throw new Q2QError('INVALID_SOURCE');
    return url.href;
  }))]);
}

/** Request permits cover admission, headers and body; all bodies are bounded. */
export async function httpBytes(options: {
  pool: PermitPool; url: string; length: number; signal: AbortSignal; timeoutMs: number;
  range?: { start: bigint; end: bigint; total: bigint };
  received?(count: number): void;
}): Promise<Uint8Array> {
  const controller = new AbortController();
  const abort = () => controller.abort();
  options.signal.addEventListener('abort', abort, { once: true });
  if (options.signal.aborted) controller.abort();
  const timer = setTimeout(abort, options.timeoutMs);
  let release: (() => void) | undefined;
  let reader: ReadableStreamDefaultReader<Uint8Array> | undefined;
  let response: Response | undefined;
  try {
    release = await options.pool.acquire(controller.signal);
    const headers = new Headers();
    if (options.range) headers.set('Range', `bytes=${options.range.start}-${options.range.end}`);
    response = await fetch(options.url, { headers, signal: controller.signal, credentials: 'omit',
      mode: 'cors', cache: 'no-store', redirect: 'follow', referrerPolicy: 'no-referrer' });
    if (new URL(options.url).protocol === 'https:' && new URL(response.url).protocol !== 'https:') throw new Q2QError('HTTP_PROTOCOL_ERROR');
    if (response.status === 401 || response.status === 403) throw new Q2QError('AUTH_REQUIRED');
    if (response.status === 416) throw new Q2QError('SOURCE_CHANGED');
    if (!response.ok) throw new Q2QError('HTTP_STATUS', response.status === 429 || response.status >= 500);
    if (options.range) {
      if (response.status === 200) {
        if (options.range.start !== 0n || options.range.end + 1n !== options.range.total) throw new Q2QError('HTTP_RANGE_UNSUPPORTED');
      } else {
        const match = /^bytes (\d+)-(\d+)\/(\d+)$/.exec(response.headers.get('Content-Range') ?? '');
        if (response.status !== 206 || !match || BigInt(match[1]!) !== options.range.start
          || BigInt(match[2]!) !== options.range.end || BigInt(match[3]!) !== options.range.total) throw new Q2QError('HTTP_PROTOCOL_ERROR');
      }
    } else if (response.status !== 200) throw new Q2QError('HTTP_PROTOCOL_ERROR');
    const encoding = response.headers.get('Content-Encoding');
    if (encoding && encoding.toLowerCase() !== 'identity') throw new Q2QError('HTTP_PROTOCOL_ERROR');
    const declared = response.headers.get('Content-Length');
    if (declared !== null && (!/^\d+$/.test(declared) || BigInt(declared) !== BigInt(options.length))) throw new Q2QError('HTTP_PROTOCOL_ERROR');
    if (!response.body) throw new Q2QError('HTTP_PROTOCOL_ERROR');
    const bytes = new Uint8Array(options.length);
    reader = response.body.getReader();
    let offset = 0;
    for (;;) {
      const chunk = await reader.read();
      if (controller.signal.aborted) throw cancelled();
      if (chunk.done) break;
      options.received?.(chunk.value.byteLength);
      if (chunk.value.length > bytes.length - offset) throw new Q2QError('HTTP_PROTOCOL_ERROR');
      bytes.set(chunk.value, offset); offset += chunk.value.length;
    }
    if (offset !== bytes.length) throw new Q2QError('HTTP_PROTOCOL_ERROR');
    return bytes;
  } catch (error) {
    if (options.signal.aborted) throw cancelled();
    if (controller.signal.aborted || !(error instanceof Q2QError)) throw new Q2QError('NETWORK_UNAVAILABLE', true);
    throw error;
  } finally {
    try {
      if (reader) { await reader.cancel().catch(() => {}); reader.releaseLock(); }
      else if (response?.body) await response.body.cancel().catch(() => {});
    } finally {
      clearTimeout(timer); options.signal.removeEventListener('abort', abort); release?.();
    }
  }
}
