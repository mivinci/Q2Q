import { z } from 'zod';
import { ProtocolError, base64, parseWire, validPeerId } from './protocol.ts';
import type { Candidate } from './state.ts';

const id = z.string().refine(validPeerId);
const cursorSchema = z.strictObject({
  hash: z.array(z.number().int().min(0).max(255)).length(32), requester: id,
  pivot: id.nullable(), after: id.nullable(), wrapped: z.boolean()
});
type Cursor = z.infer<typeof cursorSchema>;
export type Reference = { peer_id: string; revision: string };
export interface SelectionSource {
  page(after: string | null, count: number): Promise<Reference[]>;
  candidate(reference: Reference): Promise<Candidate | null>;
}
export type SelectionQuery = {
  hash: string; requester: string; limit: number; cursor?: string;
  start: string; deadline: number; rpcBudget?: number; punches: ReadonlyMap<string, {url: string; public_key: string}>;
};

function encode(cursor: Cursor): string {
  return 'v1.' + base64(new TextEncoder().encode(JSON.stringify(cursor)))
    .replaceAll('+', '-').replaceAll('/', '_').replace(/=+$/, '');
}
function initial(query: SelectionQuery): Cursor {
  const invalid = (): never => { throw new ProtocolError('bad_request'); };
  if (!/^sha256:[0-9a-f]{64}$/.test(query.hash) || !validPeerId(query.requester)) return invalid();
  const hash = Array.from(query.hash.slice(7).match(/../g)!, value => parseInt(value, 16));
  const raw = query.cursor;
  if (raw === undefined) {
    if (!validPeerId(query.start)) return invalid();
    return { hash, requester: query.requester, pivot: query.start, after: query.start, wrapped: false };
  }
  if (raw.length > 1024) return invalid();
  if (!raw.startsWith('v1.')) {
    if (!validPeerId(raw)) return invalid();
    return { hash, requester: query.requester, pivot: null, after: raw, wrapped: false };
  }
  try {
    const encoded = raw.slice(3);
    if (!/^[A-Za-z0-9_-]+$/.test(encoded)) return invalid();
    const bytes = Uint8Array.from(atob(encoded.replaceAll('-', '+').replaceAll('_', '/')), char => char.charCodeAt(0));
    const text = new TextDecoder('utf-8', { fatal: true }).decode(bytes);
    parseWire(text); // Reject duplicate fields before ordinary parsing of bounded byte values.
    const cursor = cursorSchema.parse(JSON.parse(text));
    // Reject noncanonical trailing bits, but accept JSON whitespace/key order as Serde does.
    if (base64(bytes).replaceAll('+', '-').replaceAll('/', '_').replace(/=+$/, '') !== encoded ||
      JSON.stringify(cursor.hash) !== JSON.stringify(hash) || cursor.requester !== query.requester ||
      (cursor.wrapped && cursor.pivot === null) ||
      (!cursor.wrapped && cursor.pivot !== null && cursor.after === null) ||
      (cursor.pivot !== null && cursor.after !== null &&
        (cursor.wrapped ? cursor.after > cursor.pivot : cursor.after < cursor.pivot))) return invalid();
    return cursor;
  } catch { return invalid(); }
}

// Continuations describe the last consumed position, never a prefetched batch end.
// Authoritative checks are sequential (one in flight), so timing out cannot fan out
// additional work. The host must bound each RPC by the remaining request deadline.
export async function select(query: SelectionQuery, source: SelectionSource): Promise<{peers: Candidate[]; next_cursor: string | null}> {
  if (!Number.isInteger(query.limit) || query.limit < 1 || query.limit > 50) throw new ProtocolError('bad_request');
  const cursor = initial(query), peers: Candidate[] = [];
  let scanned = 0, verified = 0, calls = 0;
  const continuation = () => ({ peers, next_cursor: encode(cursor) });
  while (true) {
    if (scanned >= 512 || calls >= (query.rpcBudget ?? 80) || Date.now() >= query.deadline) return continuation();
    const count = Math.min(64, 512 - scanned);
    calls++;
    const batch = await source.page(cursor.after, count);
    for (const reference of batch) {
      if (cursor.wrapped && cursor.pivot !== null && reference.peer_id > cursor.pivot) return { peers, next_cursor: null };
      if (reference.peer_id === query.requester) {
        scanned++; cursor.after = reference.peer_id; continue;
      }
      if (verified >= 64 || calls >= (query.rpcBudget ?? 80) || Date.now() >= query.deadline) return continuation();
      calls++; verified++;
      const candidate = await source.candidate(reference);
      const punch = candidate && query.punches.get(candidate.punch.punch_id);
      const valid = candidate !== null && candidate.peer_id === reference.peer_id && candidate.info_hash === query.hash &&
        candidate.revision === reference.revision && punch?.url === candidate.punch.url && punch.public_key === candidate.punch.public_key;
      // Look ahead before consuming the next valid record, matching native pagination.
      if (valid && peers.length === query.limit) return continuation();
      scanned++; cursor.after = reference.peer_id;
      if (valid) peers.push(candidate);
    }
    if (batch.length < count) {
      if (cursor.pivot !== null && !cursor.wrapped) { cursor.wrapped = true; cursor.after = null; }
      else return { peers, next_cursor: null };
    }
  }
}
