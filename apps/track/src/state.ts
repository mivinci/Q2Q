import { z } from 'zod';
import type { PunchDiscoveryService } from '@q2q/punch-discovery';
import type { RateLimiter } from './rate-limit.ts';
import type { PeerState } from './peer-state.ts';
import type { FileIndex } from './file-index.ts';
import type { DiscoveryState } from './directory.ts';
import { ProtocolError, U64_MAX, decodeBase64, u64Bytes } from './protocol.ts';

export interface Bindings {
  REGISTRY?: PunchDiscoveryService;
  REGISTRY_ENVIRONMENT?: string;
  RATE_LIMIT: DurableObjectNamespace<RateLimiter>;
  TURN_URL?: string;
  TURN_SECRET?: string;
  PEERS: DurableObjectNamespace<PeerState>;
  FILES: DurableObjectNamespace<FileIndex>;
  DISCOVERY: DurableObjectNamespace<DiscoveryState>;
  MAX_PEER_HOLDINGS?: string;
  MAX_OUTBOX_EVENTS?: string;
  MAX_OUTBOX_BYTES?: string;
  MAX_FILE_PEERS?: string;
  MAX_ANNOUNCE_CHANGES?: string;
  DELIVERY_TIMEOUT_MS?: string;
  REQUEST_TIMEOUT_MS?: string;
}
const positive = (fallback: number, max: number) => z.coerce.number().int().min(1).max(max).default(fallback);
const limitsSchema = z.object({
  MAX_PEER_HOLDINGS: positive(4096, 100000), MAX_OUTBOX_EVENTS: positive(512, 10000),
  MAX_OUTBOX_BYTES: positive(524288, 16777216), MAX_FILE_PEERS: positive(10000, 1000000),
  MAX_ANNOUNCE_CHANGES: positive(128, 512), DELIVERY_TIMEOUT_MS: positive(1000, 5000),
  REQUEST_TIMEOUT_MS: positive(5000, 10000)
});
export type Limits = z.infer<typeof limitsSchema>;
export const limits = (env: unknown): Limits => limitsSchema.parse(env);
export const seconds = () => Math.floor(Date.now() / 1000);
export const unavailable = (): never => { throw new ProtocolError('storage_unavailable'); };
export function checked<T>(schema: z.ZodType<T>, value: unknown): T {
  const result = schema.safeParse(value);
  if (!result.success) throw new ProtocolError('bad_request');
  return result.data;
}
export const peerSchema = z.string().regex(/^[A-Z2-7]{52}[qpzry9x8gf2tvdw0s3jn54khce6mua7l]{6}$/);
export const hashSchema = z.string().regex(/^sha256:[0-9a-f]{64}$/);
export const decimalSchema = z.string().regex(/^(0|[1-9][0-9]{0,19})$/).refine(value => BigInt(value) <= U64_MAX);
export const timeSchema = z.number().int().min(0).max(Number.MAX_SAFE_INTEGER);
const queryContextSchema = z.object({requester: peerSchema, info_hash: hashSchema, now: timeSchema, deadline: timeSchema});
export type QueryContext = z.infer<typeof queryContextSchema>;
export function queryTime(input: QueryContext, hash: string, config: Limits): number {
  const value = checked(queryContextSchema, input), now = seconds();
  if (value.info_hash !== hash || value.now > now || value.now < now - Math.ceil(config.REQUEST_TIMEOUT_MS / 1000) - 1 ||
    value.deadline > value.now * 1000 + 1000 + config.REQUEST_TIMEOUT_MS) throw new ProtocolError('bad_request');
  if (value.deadline <= Date.now()) unavailable();
  return value.now;
}
export const punchSchema = z.object({
  punch_id: z.string().min(1).max(128), url: z.string().max(2048).url().refine(value => new URL(value).protocol === 'quic:'),
  public_key: z.string().refine(value => { try { decodeBase64(value, 32); return true; } catch { return false; } })
});
export type Punch = z.infer<typeof punchSchema>;
export const eventSchema = z.object({
  peer_id: peerSchema, info_hash: hashSchema, revision: decimalSchema,
  deleted: z.boolean(), complete: z.boolean(), expires_at: timeSchema
});
export type IndexEvent = z.infer<typeof eventSchema>;
export type Session = {
  peer_id: string; public_key: string; session_key: string; last_seq: string;
  punch: Punch; expires_at: number; generation: string;
};
export type Holding = { info_hash: string; revision: string; complete: boolean; expires_at: number };
export type Candidate = Holding & { peer_id: string; public_key: string; punch: Punch };

export function blob(value: bigint): ArrayBuffer { return Uint8Array.from(u64Bytes(value)).buffer; }
export function integer(value: ArrayBuffer): bigint { return new DataView(value).getBigUint64(0, false); }
export function hasTable(storage: DurableObjectStorage, table: string): boolean {
  return storage.sql.exec('SELECT name FROM sqlite_master WHERE type = ? AND name = ?', 'table', table).toArray().length > 0;
}
export function assertObject(id: DurableObjectId, namespace: DurableObjectNamespace, name: string): void {
  if (id.toString() !== namespace.idFromName(name).toString()) throw new ProtocolError('bad_request');
}

// Serialise alarm read/modify/write without holding a storage transaction across await.
// Schedule before committing an outbox entry: a crash after commit must retain a wake-up.
export class Alarms {
  private tail: Promise<void> = Promise.resolve();
  constructor(private readonly storage: DurableObjectStorage) {}
  at(when: number): Promise<void> {
    const task = this.tail.catch(() => {}).then(async () => {
      const current = await this.storage.getAlarm();
      if (current === null || current > when) await this.storage.setAlarm(when);
    });
    this.tail = task;
    return task;
  }
  reconcile(next: () => number | null): Promise<void> {
    // Writers await at() before committing. Run this synchronous state inspection
    // after earlier wake-ups, or let later writers install their own earlier wake.
    // This may postpone a timer only when the owner confirms no retry needs it.
    const task = this.tail.catch(() => {}).then(async () => {
      const when = next();
      if (when === null) await this.storage.deleteAlarm();
      else await this.storage.setAlarm(when);
    });
    this.tail = task;
    return task;
  }
}

export async function within<T>(promise: Promise<T>, timeout: number): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([promise, new Promise<never>((_, reject) => {
      timer = setTimeout(() => reject(new ProtocolError('storage_unavailable')), timeout);
    })]);
  } finally { if (timer !== undefined) clearTimeout(timer); }
}
