import { DurableObject } from 'cloudflare:workers';
import { DISCOVERY_NAME } from './directory.ts';
import { z } from 'zod';
import { ProtocolError, U64_MAX, base64, decodeBase64, parseTicket, verifyAnnounce, verifySession } from './protocol.ts';
import { Alarms, assertObject, blob, checked, decimalSchema, hasTable, hashSchema, integer, limits, peerSchema, punchSchema, queryTime, seconds, unavailable, within } from './state.ts';
import type { Bindings, Candidate, IndexEvent, Limits, Punch, QueryContext, Session } from './state.ts';

type SessionRow = { peer_id: string; public_key: string; session_key: string; seq: ArrayBuffer; punch: string; expires_at: number; generation: ArrayBuffer };
type HoldingRow = { info_hash: string; revision: ArrayBuffer; complete: number; expires_at: number };
type PendingRow = { info_hash: string; revision: ArrayBuffer; payload: string; bytes: number };
export type NewSession = Omit<Session, 'last_seq' | 'generation'>;

/** Synchronous storage operations. Crypto/network work belongs outside these transactions. */
export class PeerStore {
  constructor(readonly storage: DurableObjectStorage, readonly config: Limits) {}
  private ready(): boolean {
    if (!hasTable(this.storage, 'peer_meta')) return false;
    if (this.storage.sql.exec<{ version: number }>('SELECT version FROM peer_meta WHERE id=1').one().version !== 1) unavailable();
    return true;
  }
  private init(): void {
    if (this.ready()) return;
    this.storage.sql.exec(`
      CREATE TABLE peer_meta(id INTEGER PRIMARY KEY CHECK(id=1), version INTEGER NOT NULL, counter BLOB NOT NULL, failures INTEGER NOT NULL);
      CREATE TABLE session(id INTEGER PRIMARY KEY CHECK(id=1), peer_id TEXT NOT NULL, public_key TEXT NOT NULL,
        session_key TEXT NOT NULL, seq BLOB NOT NULL, punch TEXT NOT NULL, expires_at INTEGER NOT NULL, generation BLOB NOT NULL);
      CREATE TABLE holdings(info_hash TEXT PRIMARY KEY, revision BLOB NOT NULL, complete INTEGER NOT NULL, expires_at INTEGER NOT NULL);
      CREATE INDEX holdings_expiry ON holdings(expires_at);
      CREATE TABLE outbox(info_hash TEXT PRIMARY KEY, revision BLOB NOT NULL, payload TEXT NOT NULL, bytes INTEGER NOT NULL);
    `);
    this.storage.sql.exec('INSERT INTO peer_meta VALUES(1,1,?,0)', blob(0n));
  }
  session(): Session | null {
    if (!this.ready()) return null;
    const row = this.storage.sql.exec<SessionRow>('SELECT * FROM session WHERE id=1').toArray()[0];
    return row ? { peer_id: row.peer_id, public_key: row.public_key, session_key: row.session_key,
      last_seq: integer(row.seq).toString(), punch: JSON.parse(row.punch) as Punch,
      expires_at: row.expires_at, generation: integer(row.generation).toString() } : null;
  }
  announce(record: NewSession, add: string[], del: string[], complete: string[]): { generation: string; targets: string[] } {
    const changes = new Map<string, boolean>();
    for (const hash of del) changes.set(hash, true);
    for (const hash of add) changes.set(hash, false);
    if (changes.size > this.config.MAX_ANNOUNCE_CHANGES) unavailable();
    return this.storage.transactionSync(() => {
      this.init();
      const previous = this.session();
      if (previous && previous.peer_id !== record.peer_id) unavailable();
      let counter = integer(this.storage.sql.exec<{counter: ArrayBuffer}>('SELECT counter FROM peer_meta WHERE id=1').one().counter);
      const next = () => { if (counter === U64_MAX) unavailable(); return ++counter; };
      const generation = next();
      const completed = new Set(complete);
      for (const [hash, deleted] of changes) {
        const revision = next();
        const event: IndexEvent = { peer_id: record.peer_id, info_hash: hash, revision: revision.toString(),
          deleted, complete: !deleted && completed.has(hash), expires_at: record.expires_at };
        if (deleted) this.storage.sql.exec('DELETE FROM holdings WHERE info_hash=?', hash);
        else this.storage.sql.exec('INSERT OR REPLACE INTO holdings VALUES(?,?,?,?)', hash, blob(revision), Number(event.complete), event.expires_at);
        const payload = JSON.stringify(event);
        // Coalescing by file is safe: delivery acks compare the revision before deleting.
        this.storage.sql.exec('INSERT OR REPLACE INTO outbox VALUES(?,?,?,?)', hash, blob(revision), payload, new TextEncoder().encode(payload).length);
      }
      const holdings = this.storage.sql.exec<{n: number}>('SELECT count(*) AS n FROM holdings').one().n;
      const pending = this.storage.sql.exec<{n: number; bytes: number}>('SELECT count(*) AS n, coalesce(sum(bytes),0) AS bytes FROM outbox').one();
      if (holdings > this.config.MAX_PEER_HOLDINGS || pending.n > this.config.MAX_OUTBOX_EVENTS || pending.bytes > this.config.MAX_OUTBOX_BYTES) unavailable();
      this.storage.sql.exec('UPDATE peer_meta SET counter=? WHERE id=1', blob(counter));
      this.storage.sql.exec('INSERT OR REPLACE INTO session VALUES(1,?,?,?,?,?,?,?)', record.peer_id, record.public_key,
        record.session_key, blob(0n), JSON.stringify(record.punch), record.expires_at, blob(generation));
      return { generation: generation.toString(), targets: [...changes.keys()] };
    });
  }
  advance(key: string, seq: bigint, renew: number | null, now: number): void {
    this.storage.transactionSync(() => {
      const session = this.session();
      if (!session || session.session_key !== key || session.expires_at <= now) throw new ProtocolError('unknown_session');
      if (seq <= BigInt(session.last_seq)) throw new ProtocolError('bad_mac');
      this.storage.sql.exec('UPDATE session SET seq=?, expires_at=? WHERE id=1', blob(seq), renew ?? session.expires_at);
    });
  }
  holding(hash: string, now: number): Candidate | null {
    const session = this.session();
    if (!session || session.expires_at <= now) return null;
    const row = this.storage.sql.exec<HoldingRow>('SELECT * FROM holdings WHERE info_hash=? AND expires_at>?', hash, now).toArray()[0];
    return row ? { info_hash: hash, revision: integer(row.revision).toString(), complete: Boolean(row.complete),
      expires_at: row.expires_at, peer_id: session.peer_id, public_key: session.public_key, punch: session.punch } : null;
  }
  pending(limit = 32): IndexEvent[] {
    if (!this.ready()) return [];
    return this.storage.sql.exec<PendingRow>('SELECT * FROM outbox ORDER BY revision LIMIT ?', limit).toArray().map(row => JSON.parse(row.payload) as IndexEvent);
  }
  ack(event: IndexEvent): void {
    if (this.ready()) this.storage.sql.exec('DELETE FROM outbox WHERE info_hash=? AND revision=?', event.info_hash, blob(BigInt(event.revision)));
  }
  pendingCount(): number { return this.ready() ? this.storage.sql.exec<{n:number}>('SELECT count(*) AS n FROM outbox').one().n : 0; }
  confirmed(targets: string[]): boolean {
    return !this.ready() || targets.every(hash => this.storage.sql.exec('SELECT info_hash FROM outbox WHERE info_hash=?', hash).toArray().length === 0);
  }
  failures(failed: boolean): number {
    if (!this.ready()) return 0;
    this.storage.sql.exec('UPDATE peer_meta SET failures=CASE WHEN ? THEN min(failures+1,10) ELSE 0 END WHERE id=1', Number(failed));
    return this.storage.sql.exec<{failures:number}>('SELECT failures FROM peer_meta WHERE id=1').one().failures;
  }
  cleanup(now: number): void {
    if (!this.ready()) return;
    this.storage.transactionSync(() => {
      this.storage.sql.exec('DELETE FROM session WHERE expires_at<=?', now);
      this.storage.sql.exec('DELETE FROM holdings WHERE info_hash IN (SELECT info_hash FROM holdings WHERE expires_at<=? LIMIT 128)', now);
      // Keep the global counter even when all holdings expire: a delayed event must not reuse a revision.
    });
  }
  nextExpiry(): number | null {
    if (!this.ready()) return null;
    const row = this.storage.sql.exec<{expiry:number|null}>(`SELECT min(expires_at) AS expiry FROM
      (SELECT expires_at FROM session UNION ALL SELECT expires_at FROM holdings)`).one();
    return row.expiry;
  }
}

const announceSchema = z.object({ token: z.string().max(65536), peer_id: peerSchema,
  add: z.array(hashSchema).max(512), del: z.array(hashSchema).max(512), complete: z.array(hashSchema).max(512), ttl: decimalSchema, punch: punchSchema });
const authSchema = z.object({ peer_id: peerSchema, seq: decimalSchema, timestamp: decimalSchema, mac: z.string().max(128) });

export class PeerState extends DurableObject<Bindings> {
  readonly #store: PeerStore;
  readonly #wake: Alarms;
  readonly #config: Limits;
  #flushing?: Promise<void>;
  readonly #active = new Map<string, Promise<unknown>>();
  constructor(ctx: DurableObjectState, env: Bindings) {
    super(ctx, env);
    this.#config = limits(env);
    this.#store = new PeerStore(ctx.storage, this.#config);
    this.#wake = new Alarms(ctx.storage);
  }
  async announce(input: z.input<typeof announceSchema>): Promise<{session_key: string; expires_at: number}> {
    const request = checked(announceSchema, input);
    assertObject(this.ctx.id, this.env.PEERS, `peer:${request.peer_id}`);
    const ticket = parseTicket(request.token);
    if (ticket.iss !== request.peer_id) throw new ProtocolError('bad_request');
    const now = seconds();
    await verifyAnnounce(ticket, BigInt(now));
    const ttl = Number(BigInt(request.ttl) < 3600n ? BigInt(request.ttl) : 3600n);
    const key = base64(crypto.getRandomValues(new Uint8Array(32)));
    // An alarm is durable before the SQL commit; failure here leaves the old session untouched.
    await this.#wake.at(Date.now() + 1000);
    const operation = this.#store.announce({ peer_id: request.peer_id, public_key: ticket.pub, session_key: key,
      expires_at: now + ttl, punch: request.punch }, request.add, request.del, request.complete);
    const deadline = Date.now() + this.#config.REQUEST_TIMEOUT_MS;
    for (let round = 0; round < 4; round++) {
      const before = this.#store.pendingCount();
      if (this.#store.confirmed(operation.targets) || Date.now() >= deadline) break;
      await this.#flush(deadline);
      if (this.#store.session()?.generation !== operation.generation) unavailable();
      if (this.#store.pendingCount() >= before || this.#active.size) break;
    }
    await this.#wake.reconcile(() => {
      if (this.#store.pendingCount()) return Date.now() + 1000;
      const expiry = this.#store.nextExpiry();
      return expiry === null ? null : Math.max(Date.now() + 1000, expiry * 1000);
    });
    // Check after the scheduling await: a newer announce may have rotated the key.
    const current = this.#store.session();
    if (current?.generation !== operation.generation || !this.#store.confirmed(operation.targets)) unavailable();
    return { session_key: key, expires_at: now + ttl };
  }
  async authorizeQuery(input: z.input<typeof authSchema>, hash: string, deadline: number): Promise<QueryContext> {
    checked(hashSchema, hash);
    const context = {requester: input.peer_id, info_hash: hash, now: seconds(), deadline};
    queryTime(context, hash, this.#config);
    await this.#authenticate(input, false, false);
    return context;
  }
  heartbeat(input: z.input<typeof authSchema>): Promise<{expires_at:number}> {
    return this.#authenticate(input, true, true);
  }
  authenticate(input: z.input<typeof authSchema>, renew: boolean): Promise<{expires_at:number}> {
    return this.#authenticate(input, renew, false);
  }
  async #authenticate(input: z.input<typeof authSchema>, renew: boolean, discover: boolean): Promise<{expires_at:number}> {
    const request = checked(authSchema, input);
    checked(z.boolean(), renew);
    assertObject(this.ctx.id, this.env.PEERS, `peer:${request.peer_id}`);
    const session = this.#store.session();
    const now = seconds();
    if (!session || session.expires_at <= now) throw new ProtocolError('unknown_session');
    await verifySession(decodeBase64(session.session_key, 32), request.peer_id, BigInt(request.seq), BigInt(request.timestamp),
      request.mac, BigInt(session.last_seq), BigInt(now));
    if (discover) {
      // Match native error precedence: authenticate before consulting discovery,
      // then compare the original key again when atomically advancing the session.
      const snapshot = await within(this.env.DISCOVERY.getByName(DISCOVERY_NAME).snapshot(), this.#config.REQUEST_TIMEOUT_MS);
      const current = snapshot.nodes.find(node => node.punch_id === session.punch.punch_id);
      if (!current || current.url !== session.punch.url || current.public_key !== session.punch.public_key) throw new ProtocolError('unknown_session');
    }
    const expiry = renew ? now + 3600 : null;
    // Renewing needs a durable expiry wake-up, not an immediate cleanup pass.
    // Alarms.at preserves any earlier holding expiry or pending outbox retry.
    if (expiry !== null) await this.#wake.at(Math.max(Date.now() + 1000, expiry * 1000));
    this.#store.advance(session.session_key, BigInt(request.seq), expiry, now);
    return { expires_at: expiry ?? session.expires_at };
  }
  async candidate(hash: string, revision: string, context?: QueryContext): Promise<Candidate | null> {
    checked(hashSchema, hash); checked(decimalSchema, revision);
    const value = this.#store.holding(hash, context === undefined ? seconds() : queryTime(context, hash, this.#config));
    return value?.revision === revision ? value : null;
  }
  #flush(deadline = Date.now() + this.#config.REQUEST_TIMEOUT_MS): Promise<void> {
    if (this.#flushing) return this.#flushing;
    const task = this.#deliver(deadline).finally(() => { if (this.#flushing === task) this.#flushing = undefined; });
    this.#flushing = task;
    return task;
  }
  async #deliver(deadline: number): Promise<void> {
    const events = this.#store.pending(32).filter(event => !this.#active.has(event.info_hash));
    for (let offset = 0; offset < events.length; offset += 4) {
      if (Date.now() >= deadline) break;
      await Promise.all(events.slice(offset, offset + 4).map(async event => {
        if (this.#active.size >= 8) return;
        const rpc = this.env.FILES.getByName(`file:${event.info_hash}`).apply(event);
        this.#active.set(event.info_hash, rpc);
        // A local deadline doesn't cancel the RPC. Hold its slot until it really settles;
        // late writes and replayed deliveries remain safe through the persisted revision.
        void rpc.then(() => this.#active.delete(event.info_hash), () => this.#active.delete(event.info_hash));
        try { await within(rpc, Math.max(1, Math.min(this.#config.DELIVERY_TIMEOUT_MS, deadline - Date.now()))); this.#store.ack(event); } catch { /* alarm retries */ }
      }));
    }
  }
  async alarm(): Promise<void> {
    this.#store.cleanup(seconds());
    // Re-arm before awaiting delivery, including when a transport never responds.
    if (this.#store.pendingCount()) await this.#wake.at(Date.now() + 30000);
    await this.#flush();
    const failed = this.#store.pendingCount() > 0;
    const failures = this.#store.failures(failed);
    if (failed) await this.#wake.at(Date.now() + Math.min(30000, 1000 * 2 ** failures) + Math.floor(Math.random() * 250));
    const expiry = this.#store.nextExpiry();
    if (expiry !== null) await this.#wake.at(Math.max(Date.now() + 1000, expiry * 1000));
  }
}
