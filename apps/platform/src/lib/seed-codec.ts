import init, { decode_seed } from '@q2q/wasm';
import wasmUrl from '@q2q/wasm/module.wasm?url';

export interface DecodedSeed {
  name: string | null; size: string; blockSize: number; blockCount: string;
  infoHash: string; root: string; inlineProof: boolean;
  tracks: string[]; sources: string[]; proofSources: string[];
}
let initialized: ReturnType<typeof init> | undefined;
export async function initializeSeedCodec() {
  await (initialized ??= init({ module_or_path: wasmUrl }).catch((cause) => { initialized = undefined; throw cause; }));
}
export async function inspectSeed(bytes: Uint8Array): Promise<DecodedSeed> {
  await initializeSeedCodec();
  const seed: DecodedSeed = JSON.parse(decode_seed(bytes));
  for (const key of ['tracks', 'sources', 'proofSources'] as const) seed[key] = seed[key].map((value) => sourceSite(value) ?? '…');
  return seed;
}

// Keep the endpoint readable while never rendering credentials or authorization parameters.
export function sourceSite(value: string): string | null {
  try {
    const url = new URL(value);
    return ['http:', 'https:', 'ws:', 'wss:'].includes(url.protocol) ? `${url.origin}${url.pathname}` : null;
  } catch { return null; }
}
