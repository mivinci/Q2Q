import { Hono } from 'hono';
import { acceptsNewPeer } from '@q2q/punch-discovery';
import { parse } from 'lossless-json';
import { bodyLimit } from 'hono/body-limit';
import { z } from 'zod';
import { DISCOVERY_NAME } from './directory.ts';
import { ProtocolError, stringifyWire, ticketSchema, uint64, verifyAnnounce, peerId, turnCredential, validPeerId } from './protocol.ts';
import { limits, seconds, within } from './state.ts';
import type { Bindings } from './state.ts';
import type { DirectorySnapshot } from './directory.ts';
import { select } from './selection.ts';
import { rateObjectName } from './rate-key.ts';

const announceSchema = z.object({ token: ticketSchema, ttl: uint64,
  add: z.array(z.string()).default([]), del: z.array(z.string()).default([]), complete: z.array(z.string()).default([]) });
const heartbeatSchema = z.object({seq: uint64, timestamp: uint64, mac: z.string()});
const fail = (code: string): never => {throw new ProtocolError(code);};
function hash(value: string): string {
  if (!/^sha256:[0-9a-fA-F]{64}$/.test(value)) return fail('bad_request');
  return value.toLowerCase();
}
function json<T>(schema: z.ZodType<T>, body: string): T {
  let value: unknown;
  try { value = parse(body); } catch (error) { return fail(error instanceof Error && error.message.startsWith('Duplicate key ') ? 'unprocessable_entity' : 'bad_request'); }
  const result = schema.safeParse(value);
  if (!result.success) return fail('unprocessable_entity');
  return result.data;
}
async function assignment(snapshot: DirectorySnapshot, peer: string) {
  const key = await crypto.subtle.importKey('raw', new TextEncoder().encode('q2q-punch-assignment-v1'), {name: 'HMAC', hash: 'SHA-256'}, false, ['sign']);
  const eligible = snapshot.nodes.filter(node => acceptsNewPeer(node) && node.expires_at > seconds());
  let chosen: typeof eligible[number] | undefined, highest = '';
  for (const node of eligible) {
    const score = Array.from(new Uint8Array(await crypto.subtle.sign('HMAC', key, new TextEncoder().encode(`${peer}\0${node.punch_id}`))), byte => byte.toString(16).padStart(2, '0')).join('');
    if (score > highest || (score === highest && node.punch_id > (chosen?.punch_id ?? ''))) {highest = score; chosen = node;}
  }
  if (!chosen) return fail('no_available_punch');
  return {punch_id: chosen.punch_id, url: chosen.url, public_key: chosen.public_key};
}

export const api = new Hono<{Bindings: Bindings; Variables: {deadline: number}}>();
api.use('*', async (context, next) => {
  context.set('deadline', Date.now() + limits(context.env).REQUEST_TIMEOUT_MS);
  context.header('Cache-Control', 'no-store');
  context.header('Access-Control-Allow-Origin', '*');
  await next();
});
api.use('*', bodyLimit({maxSize: 2 * 1024 * 1024, onError: context => context.json({code: 'payload_too_large'}, 413)}));
api.use('*', async (context, next) => {
  if (['PUT', 'POST'].includes(context.req.method) && !/^application\/(?:json|[\w.+-]+\+json)(?:\s*;|$)/i.test(context.req.header('content-type') ?? '')) return context.json({code: 'unsupported_media_type'}, 415);
  await next();
});
api.use('*', async (context, next) => {
  // Cloudflare supplies/overwrites this header at the public edge. Missing local
  // addresses share a conservative bucket instead of trusting forwarded headers.
  const address = context.req.header('CF-Connecting-IP') ?? 'local';
  const name = await rateObjectName(address);
  const kind = context.req.method === 'PUT' ? 'announce' : 'session';
  const accepted = await within(context.env.RATE_LIMIT.getByName(name).take(kind), Math.max(1, context.get('deadline') - Date.now()));
  if (!accepted) {context.header('Retry-After', '6'); return context.json({code: 'rate_limited'}, 429);}
  await next();
});
api.put('/peers/:peer/files', async context => {
  const deadline = context.get('deadline');
  const rpc = <T>(promise: Promise<T>) => within(promise, Math.max(1, deadline - Date.now()));
  const input = json(announceSchema, await context.req.text()), peer = context.req.param('peer');
  if (input.token.iss !== peer) return fail('bad_request');
  await verifyAnnounce(input.token, BigInt(seconds()));
  const add = input.add.map(hash), del = input.del.map(hash), complete = input.complete.map(hash);
  // Validate deployment secrets before changing durable session state.
  if (!context.env.TURN_URL || !context.env.TURN_SECRET) return fail('configuration_error');
  const snapshot = await rpc(context.env.DISCOVERY.getByName(DISCOVERY_NAME).snapshot());
  const punch = await assignment(snapshot, peer);
  const session = await rpc(context.env.PEERS.getByName(`peer:${peer}`).announce({peer_id: peer, token: stringifyWire(input.token),
    ttl: input.ttl.toString(), add, del, complete, punch}));
  const expires_at = seconds() + 3600, username = `${expires_at}:${peer}`;
  return context.json({...session, punch, heartbeat_interval: 30,
    turn: {url: context.env.TURN_URL, username, credential: await turnCredential(context.env.TURN_SECRET, username), expires_at}});
});
api.post('/peers/:peer/heartbeat', async context => {
  const deadline = context.get('deadline');
  const rpc = <T>(promise: Promise<T>) => within(promise, Math.max(1, deadline - Date.now()));
  const input = json(heartbeatSchema, await context.req.text()), peer = context.req.param('peer');
  if (!validPeerId(peer)) return fail('unknown_session');
  return context.json(await rpc(context.env.PEERS.getByName(`peer:${peer}`).heartbeat({peer_id: peer, seq: input.seq.toString(), timestamp: input.timestamp.toString(), mac: input.mac})));
});
api.get('/files/:hash/peers', async context => {
  const info_hash = hash(context.req.param('hash')), query = context.req.query();
  const parameters = new URL(context.req.url).searchParams;
  for (const field of ['peer_id', 'seq', 'timestamp', 'mac', 'limit', 'cursor']) if (parameters.getAll(field).length > 1) return fail('bad_request');
  const decimal = (value: string): string => {
    if (!/^\+?[0-9]+$/.test(value)) return fail('bad_request');
    const canonical = value.replace(/^\+?0*/, '') || '0';
    if (canonical.length > 20 || BigInt(canonical) > ((1n << 64n) - 1n)) return fail('bad_request');
    return BigInt(canonical).toString();
  };
  for (const field of ['seq', 'timestamp', 'limit'] as const) if (query[field] !== undefined) decimal(query[field]);
  if (!query.peer_id || query.seq === undefined || query.timestamp === undefined || query.mac === undefined) return fail('unknown_session');
  if (!validPeerId(query.peer_id)) return fail('unknown_session');
  const seq = decimal(query.seq), timestamp = decimal(query.timestamp);
  const limit = query.limit === undefined ? 10 : Number(BigInt(decimal(query.limit)) > 50n ? 50n : BigInt(decimal(query.limit)) || 1n);
  const deadline = context.get('deadline');
  const rpc = <T>(promise: Promise<T>) => within(promise, Math.max(1, deadline - Date.now()));
  const authenticated = await rpc(context.env.PEERS.getByName(`peer:${query.peer_id}`).authorizeQuery({peer_id: query.peer_id, seq, timestamp, mac: query.mac}, info_hash, deadline));
  const snapshot = await rpc(context.env.DISCOVERY.getByName(DISCOVERY_NAME).snapshot());
  const selected = await select({hash: info_hash, requester: query.peer_id, limit, cursor: query.cursor, deadline, rpcBudget: 77,
    start: await peerId(crypto.getRandomValues(new Uint8Array(32))), punches: new Map(snapshot.nodes.filter(node => node.expires_at > seconds()).map(node => [node.punch_id, node]))}, {
    page: (after, count) => rpc(context.env.FILES.getByName(`file:${info_hash}`).page(info_hash, after, count, authenticated)),
    candidate: reference => rpc(context.env.PEERS.getByName(`peer:${reference.peer_id}`).candidate(info_hash, reference.revision, authenticated))
  });
  return context.json({peers: selected.peers.map(peer => ({peer_id: peer.peer_id, public_key: peer.public_key, punch: peer.punch, complete: peer.complete})), next_cursor: selected.next_cursor});
});
