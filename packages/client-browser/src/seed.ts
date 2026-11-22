import { Q2QError } from './error.ts';
import { initializeWasm } from './block-verifier.ts';
import { MAX_BLOCKS } from './checkpoint-codec.ts';
import { abortable, cancelled } from './http.ts';
import type { ContentSource, SourceReader } from './storage.ts';
import type { Seed, ContentInfo } from './client-types.ts';

export interface SeedData { bytes: Uint8Array; contentUrls: readonly string[]; proofUrls: readonly string[]; inline: boolean }
const seeds = new WeakMap<Seed, SeedData>();
export function seedData(seed: Seed): SeedData {
  const value = seeds.get(seed);
  if (!value) throw new Q2QError('INVALID_SEED');
  return value;
}
export async function parseSeed(input: Uint8Array | ContentSource, signal: AbortSignal, wasmUrl?: string | URL): Promise<Seed> {
  const limit = 17 * 1024 * 1024;
  let bytes: Uint8Array;
  if (signal.aborted) throw cancelled();
  if (input instanceof Uint8Array) {
    if (input.length > limit) throw new Q2QError('RESOURCE_LIMIT');
    bytes = new Uint8Array(input);
  } else {
    if (typeof input.info?.size !== 'bigint' || input.info.size < 0n || input.info.size > BigInt(limit)) throw new Q2QError('RESOURCE_LIMIT');
    let reader: SourceReader | undefined;
    const opening = input.open();
    void opening.then(value => { if (signal.aborted) void value.close().catch(() => {}); }, () => {});
    try {
      reader = await abortable(opening, signal);
      bytes = new Uint8Array(Number(input.info.size));
      for (let offset = 0; offset < bytes.length; offset += 1048576) {
        const length = Math.min(1048576, bytes.length - offset);
        const chunk = await reader.read(BigInt(offset), length, { signal });
        if (chunk.length !== length) throw new Q2QError('SOURCE_CHANGED');
        bytes.set(chunk, offset);
      }
    } finally { await reader?.close(); }
  }
  const module = await abortable(initializeWasm(wasmUrl), signal);
  if (signal.aborted) throw cancelled();
  try {
    const metadata = JSON.parse(module.decode_seed(bytes)) as {
      infoHash: string; size: string; blockSize: number; blockCount: string; name: string | null;
      inlineProof: boolean; sources: string[]; proofSources: string[]; tracks: string[];
    };
    if (BigInt(metadata.size) > BigInt(Number.MAX_SAFE_INTEGER)) throw new Q2QError('UNSUPPORTED_CAPABILITY');
    if (BigInt(metadata.blockCount) > BigInt(MAX_BLOCKS)) throw new Q2QError('RESOURCE_LIMIT');
    const info: ContentInfo = Object.freeze({ id: `sha256:${metadata.infoHash}`, name: metadata.name,
      size: BigInt(metadata.size), blockSize: metadata.blockSize, blockCount: BigInt(metadata.blockCount) });
    const seed: Seed = Object.freeze({ info,
      hints: Object.freeze({ contentSources: metadata.sources.length, proofSources: metadata.proofSources.length, tracks: metadata.tracks.length }),
      toBytes: () => new Uint8Array(bytes) });
    seeds.set(seed, { bytes, contentUrls: Object.freeze([...metadata.sources]), proofUrls: Object.freeze([...metadata.proofSources]), inline: metadata.inlineProof });
    return seed;
  } catch (error) { if (error instanceof Q2QError) throw error; throw new Q2QError('INVALID_SEED'); }
}
