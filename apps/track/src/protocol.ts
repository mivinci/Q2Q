import { isLosslessNumber, parse, stringify } from 'lossless-json';
import { z } from 'zod';

export const U64_MAX = (1n << 64n) - 1n;
const I64_MAX = (1n << 63n) - 1n;
const utf8 = new TextEncoder();

export class ProtocolError extends Error {
  code: string;
  constructor(code: string) { super(code); this.code = code; }
}
const fail = (code: string): never => { throw new ProtocolError(code); };

// Preserve the original numeric token: Number -> BigInt cannot undo JSON rounding.
// Duplicate keys are rejected by the parser instead of silently using the last value.
export function parseWire(text: string): unknown {
  try { return parse(text); } catch { return fail('bad_request'); }
}
export function stringifyWire(value: unknown): string {
  const text = stringify(value);
  if (text === undefined) return fail('bad_request');
  return text;
}
export function u64(value: unknown): bigint {
  if (!isLosslessNumber(value) || value.value.length > 20 || !/^(0|[1-9][0-9]*)$/.test(value.value)) return fail('bad_request');
  const integer = BigInt(value.value);
  if (integer > U64_MAX) return fail('bad_request');
  return integer;
}
export const uint64 = z.unknown().transform((value, ctx) => {
  try { return u64(value); } catch {
    ctx.addIssue({ code: 'custom', message: 'expected unsigned 64-bit integer' });
    return z.NEVER;
  }
});
export const ticketSchema = z.object({
  v: uint64.refine(value => value <= 0xffff_ffffn),
  iss: z.string(), pub: z.string(), iat: uint64, exp: uint64, aud: z.string(),
  tgt: z.string().nullable().optional(), ih: z.string().nullable().optional(),
  cid: z.string().nullable().optional(), sig: z.string()
});
export type Ticket = z.infer<typeof ticketSchema>;
export function parseTicket(text: string): Ticket {
  const result = ticketSchema.safeParse(parseWire(text));
  if (!result.success) return fail('bad_request');
  return result.data;
}
export function parseHeartbeat(text: string) {
  const result = z.object({ seq: uint64, timestamp: uint64, mac: z.string() }).safeParse(parseWire(text));
  if (!result.success) return fail('bad_request');
  return result.data;
}

export function base64(bytes: Uint8Array): string {
  return btoa(Array.from(bytes, byte => String.fromCharCode(byte)).join(''));
}
export function decodeBase64(text: string, length: number): Uint8Array {
  try {
    const bytes = Uint8Array.from(atob(text), char => char.charCodeAt(0));
    // Rust STANDARD requires padding and canonical trailing bits; atob is more lenient.
    if (bytes.length !== length || base64(bytes) !== text) return fail('malformed_ticket');
    return bytes;
  } catch { return fail('malformed_ticket'); }
}
function buffer(bytes: Uint8Array): ArrayBuffer { return Uint8Array.from(bytes).buffer; }
function concat(parts: Uint8Array[]): Uint8Array {
  const result = new Uint8Array(parts.reduce((size, part) => size + part.length, 0));
  let offset = 0;
  for (const part of parts) { result.set(part, offset); offset += part.length; }
  return result;
}
function bstring(bytes: Uint8Array): Uint8Array { return concat([utf8.encode(`${bytes.length}:`), bytes]); }

export function canonicalTicket(ticket: Ticket): Uint8Array {
  if (ticket.iat > I64_MAX || ticket.exp > I64_MAX) return fail('malformed_ticket');
  const entries: Record<string, Uint8Array> = {
    v: utf8.encode(`i${ticket.v}e`), iss: bstring(utf8.encode(ticket.iss)),
    pub: bstring(decodeBase64(ticket.pub, 32)), iat: utf8.encode(`i${ticket.iat}e`),
    exp: utf8.encode(`i${ticket.exp}e`), aud: bstring(utf8.encode(ticket.aud))
  };
  if (ticket.tgt != null) entries.tgt = bstring(utf8.encode(ticket.tgt));
  if (ticket.ih != null) entries.ih = bstring(decodeBase64(ticket.ih, 32));
  if (ticket.cid != null) entries.cid = bstring(utf8.encode(ticket.cid));
  return concat([utf8.encode('d'), ...Object.keys(entries).sort().flatMap(key => [bstring(utf8.encode(key)), entries[key]!]), utf8.encode('e')]);
}

export async function peerId(publicKey: Uint8Array): Promise<string> {
  const digest = new Uint8Array(await crypto.subtle.digest('SHA-256', buffer(publicKey)));
  const values: number[] = [];
  let bits = 0, count = 0;
  for (const byte of digest) {
    bits = (bits << 8) | byte;
    count += 8;
    while (count >= 5) { count -= 5; values.push((bits >>> count) & 31); }
  }
  if (count) values.push((bits << (5 - count)) & 31);
  const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567';
  return values.map(value => alphabet[value]).join('') + peerChecksum(values);
}

function peerChecksum(values: number[]): string {
  const checksumAlphabet = 'qpzry9x8gf2tvdw0s3jn54khce6mua7l';
  const generators = [0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3];
  let check = 1;
  for (const value of [0, ...values, 0, 0, 0, 0, 0, 0]) {
    const top = check >>> 25;
    check = ((check & 0x1ffffff) << 5) ^ value;
    for (let i = 0; i < generators.length; i++) if ((top >>> i) & 1) check ^= generators[i]!;
  }
  check ^= 1;
  return Array.from({ length: 6 }, (_, i) => checksumAlphabet[(check >>> (5 * (5 - i))) & 31]).join('');
}

export async function verifyAnnounce(ticket: Ticket, now: bigint): Promise<void> {
  const publicKey = decodeBase64(ticket.pub, 32);
  const signature = decodeBase64(ticket.sig, 64);
  if (ticket.ih != null) decodeBase64(ticket.ih, 32);
  if (ticket.v !== 2n || ticket.iat > ticket.exp || ticket.iat > I64_MAX || ticket.exp > I64_MAX) return fail('malformed_ticket');
  if (await peerId(publicKey) !== ticket.iss) return fail('key_not_issuer');
  let valid: boolean;
  try {
    const key = await crypto.subtle.importKey('raw', buffer(publicKey), 'Ed25519', false, ['verify']);
    valid = await crypto.subtle.verify('Ed25519', key, buffer(signature), buffer(canonicalTicket(ticket)));
  } catch { return fail('bad_signature'); }
  if (!valid) return fail('bad_signature');
  if (now >= ticket.exp) return fail('expired');
  // Match the current Rust rule: remaining lifetime, not exp - iat.
  if (ticket.exp - now > 600n) return fail('lifetime_too_long');
  if (ticket.aud !== 'track' || ticket.tgt != null) return fail('wrong_audience');
}

export function u64Bytes(value: bigint): Uint8Array {
  if (value < 0n || value > U64_MAX) return fail('bad_request');
  const bytes = new Uint8Array(8);
  new DataView(bytes.buffer).setBigUint64(0, value, false);
  return bytes;
}
export function sessionPayload(peer: string, seq: bigint, timestamp: bigint): Uint8Array {
  return concat([utf8.encode(peer), u64Bytes(seq), u64Bytes(timestamp)]);
}
export async function sessionMac(key: Uint8Array, peer: string, seq: bigint, timestamp: bigint): Promise<string> {
  const imported = await crypto.subtle.importKey('raw', buffer(key), { name: 'HMAC', hash: 'SHA-256' }, false, ['sign']);
  return base64(new Uint8Array(await crypto.subtle.sign('HMAC', imported, buffer(sessionPayload(peer, seq, timestamp)))));
}
export async function verifySession(key: Uint8Array, peer: string, seq: bigint, timestamp: bigint, mac: string, lastSeq: bigint, now: bigint): Promise<void> {
  if ((timestamp > now ? timestamp - now : now - timestamp) > 60n) return fail('stale_timestamp');
  if (lastSeq - seq >= 8n) return fail('bad_mac');
  let bytes: Uint8Array;
  try { bytes = decodeBase64(mac, 32); } catch { return fail('bad_mac'); }
  const imported = await crypto.subtle.importKey('raw', buffer(key), { name: 'HMAC', hash: 'SHA-256' }, false, ['verify']);
  if (!await crypto.subtle.verify('HMAC', imported, buffer(bytes), buffer(sessionPayload(peer, seq, timestamp)))) return fail('bad_mac');
  // This only authenticates the request. Storage must atomically require seq > last_seq.
}
export async function turnCredential(secret: string, username: string): Promise<string> {
  const key = await crypto.subtle.importKey('raw', buffer(utf8.encode(secret)), { name: 'HMAC', hash: 'SHA-1' }, false, ['sign']);
  return base64(new Uint8Array(await crypto.subtle.sign('HMAC', key, buffer(utf8.encode(username)))));
}

export function validPeerId(id: string): boolean {
  if (!/^[A-Z2-7]{52}[qpzry9x8gf2tvdw0s3jn54khce6mua7l]{6}$/.test(id)) return false;
  const values = Array.from(id.slice(0, 52), char => 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567'.indexOf(char));
  return peerChecksum(values) === id.slice(52);
}
