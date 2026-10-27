import vectors from '../../../tests/fixtures/track/v1.json' with { type: 'json' };
import {
  ProtocolError, base64, canonicalTicket, decodeBase64, parseHeartbeat, parseTicket,
  peerId, sessionPayload, stringifyWire, turnCredential, verifyAnnounce, verifySession
} from '../src/protocol.ts';

function equal(actual: unknown, expected: unknown): void {
  if (actual !== expected) throw new Error(`expected ${String(expected)}, received ${String(actual)}`);
}
async function errorCode(action: () => unknown | Promise<unknown>): Promise<string | null> {
  try { await action(); return null; } catch (error) {
    if (error instanceof ProtocolError) return error.code;
    throw error;
  }
}
export const checks: { name: string; run: () => Promise<void> }[] = [
  ...vectors.identities.map(vector => ({ name: `identity/${vector.peer_id}`, run: async () => {
    equal(await peerId(decodeBase64(vector.public_key, 32)), vector.peer_id);
  } })),
  ...vectors.tickets.map(vector => ({ name: `ticket/${vector.id}`, run: async () => {
    const ticket = parseTicket(vector.body_json);
    if (vector.canonical_base64 !== null) equal(base64(canonicalTicket(ticket)), vector.canonical_base64);
    equal(await errorCode(() => verifyAnnounce(ticket, BigInt(vector.now))), vector.error);
  } })),
  ...vectors.sessions.map(vector => ({ name: `session/${vector.id}`, run: async () => {
    const key = decodeBase64(vector.key_base64, 32);
    const seq = BigInt(vector.seq), timestamp = BigInt(vector.timestamp);
    equal(base64(sessionPayload(vector.peer_id, seq, timestamp)), vector.payload_base64);
    equal(await errorCode(() => verifySession(key, vector.peer_id, seq, timestamp, vector.mac, BigInt(vector.last_seq), BigInt(vector.now))), vector.error);
  } })),
  ...vectors.integers.map(vector => ({ name: `integer/${vector.body_json}`, run: async () => {
    if (vector.value === null) {
      equal(await errorCode(() => parseHeartbeat(vector.body_json)), 'bad_request');
    } else {
      const parsed = parseHeartbeat(vector.body_json);
      equal(parsed.seq.toString(), vector.value);
      // Full u64 values stay JSON numeric tokens rather than quoted strings.
      equal(stringifyWire(parsed), vector.body_json);
    }
  } })),
  { name: 'turn/hmac-sha1', run: async () => {
    equal(await turnCredential(vectors.turn.secret, vectors.turn.username), vectors.turn.credential);
  } },
  { name: 'integer/reject-duplicate-known-field', run: async () => {
    equal(await errorCode(() => parseHeartbeat('{"seq":1,"seq":2,"timestamp":1,"mac":""}')), 'bad_request');
  } }
];

export async function runContract(): Promise<{ passed: number; errors: string[] }> {
  const errors: string[] = [];
  for (const check of checks) {
    try { await check.run(); } catch (error) { errors.push(`${check.name}: ${String(error)}`); }
  }
  return { passed: checks.length - errors.length, errors };
}
