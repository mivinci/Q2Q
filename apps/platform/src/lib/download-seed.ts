import { seed_with_network } from '@q2q/wasm';
import { initializeSeedCodec } from './seed-codec.js';
export async function downloadSeed(bytes: Uint8Array, source: string, name: string, track?: string | null) {
  await initializeSeedCodec();
  const seed = seed_with_network(bytes, source, track ?? undefined);
  const url = URL.createObjectURL(new Blob([new Uint8Array(seed).buffer], { type: 'application/octet-stream' }));
  const anchor = document.createElement('a');
  anchor.href = url; anchor.download = `${name}.q2q`; anchor.click();
  setTimeout(() => URL.revokeObjectURL(url), 60_000);
}
