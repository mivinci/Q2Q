import { DurableObject } from 'cloudflare:workers';
import { z } from 'zod';
import { Alarms, assertObject, blob, checked, eventSchema, hasTable, hashSchema, integer, limits, peerSchema, queryTime, seconds, unavailable } from './state.ts';
import type { Bindings, IndexEvent, Limits, QueryContext } from './state.ts';

type FileRow = { peer_id: string; revision: ArrayBuffer; payload: string; complete: number; expires_at: number; deleted: number; retired: number };

export class FileStore {
  constructor(readonly storage: DurableObjectStorage, readonly config: Limits) {}
  private ready(): boolean {
    if (!hasTable(this.storage, 'file_meta')) return false;
    if (this.storage.sql.exec<{version:number}>('SELECT version FROM file_meta WHERE id=1').one().version !== 1) unavailable();
    return true;
  }
  private init(): void {
    if (this.ready()) return;
    this.storage.sql.exec(`
      CREATE TABLE file_meta(id INTEGER PRIMARY KEY CHECK(id=1), version INTEGER NOT NULL);
      INSERT INTO file_meta VALUES(1,1);
      CREATE TABLE members(peer_id TEXT PRIMARY KEY, revision BLOB NOT NULL, payload TEXT NOT NULL,
        complete INTEGER NOT NULL, expires_at INTEGER NOT NULL, deleted INTEGER NOT NULL, retired INTEGER NOT NULL);
      CREATE INDEX members_expiry ON members(retired, expires_at);
    `);
  }
  apply(event: IndexEvent): 'applied' | 'duplicate' | 'stale' {
    return this.storage.transactionSync(() => {
      this.init();
      const row = this.storage.sql.exec<FileRow>('SELECT * FROM members WHERE peer_id=?', event.peer_id).toArray()[0];
      const revision = BigInt(event.revision), payload = JSON.stringify(event);
      if (row) {
        const previous = integer(row.revision);
        if (previous > revision) return 'stale';
        if (previous === revision) {
          if (row.payload !== payload) unavailable();
          return 'duplicate';
        }
      } else if (this.storage.sql.exec<{n:number}>('SELECT count(*) AS n FROM members').one().n >= this.config.MAX_FILE_PEERS) unavailable();
      this.storage.sql.exec('INSERT OR REPLACE INTO members VALUES(?,?,?,?,?,?,0)', event.peer_id, blob(revision), payload,
        Number(event.complete), event.expires_at, Number(event.deleted));
      return 'applied';
    });
  }
  page(after: string | null, limit: number, now: number): {peer_id:string; revision:string}[] {
    if (!this.ready()) return [];
    // The expiry index otherwise encourages a full live-set scan plus sorting.
    // Use the existing TEXT primary-key index to bound ordered warm-page reads;
    // no duplicate secondary index (and extra write amplification) is needed.
    return this.storage.sql.exec<Pick<FileRow, 'peer_id' | 'revision'>>(`SELECT peer_id, revision FROM members
      INDEXED BY sqlite_autoindex_members_1 WHERE peer_id>? AND deleted=0 AND retired=0
      AND expires_at>? ORDER BY peer_id COLLATE BINARY LIMIT ?`, after ?? '', now, limit).toArray()
      .map(row => ({ peer_id: row.peer_id, revision: integer(row.revision).toString() }));
  }
  cleanup(now: number): void {
    if (!this.ready()) return;
    // Keep the revision and immutable payload for replay checks, even after expiry.
    this.storage.sql.exec(`UPDATE members SET retired=1 WHERE peer_id IN
      (SELECT peer_id FROM members WHERE retired=0 AND expires_at<=? LIMIT 128)`, now);
  }
  nextExpiry(): number | null {
    if (!this.ready()) return null;
    return this.storage.sql.exec<{expiry:number|null}>('SELECT min(expires_at) AS expiry FROM members WHERE retired=0 AND deleted=0').one().expiry;
  }
}

export class FileIndex extends DurableObject<Bindings> {
  readonly #store: FileStore;
  readonly #wake: Alarms;
  constructor(ctx: DurableObjectState, env: Bindings) {
    super(ctx, env); this.#store = new FileStore(ctx.storage, limits(env)); this.#wake = new Alarms(ctx.storage);
  }
  async apply(input: IndexEvent): Promise<'applied' | 'duplicate' | 'stale'> {
    const event = checked(eventSchema, input);
    assertObject(this.ctx.id, this.env.FILES, `file:${event.info_hash}`);
    if (!event.deleted) await this.#wake.at(Math.max(Date.now() + 1000, event.expires_at * 1000));
    return this.#store.apply(event);
  }
  async page(hash: string, after: string | null, limit: number, context?: QueryContext): Promise<{peer_id:string; revision:string}[]> {
    checked(hashSchema, hash); if (after !== null) checked(peerSchema, after);
    checked(z.number().int().min(1).max(64), limit);
    assertObject(this.ctx.id, this.env.FILES, `file:${hash}`);
    return this.#store.page(after, limit, context === undefined ? seconds() : queryTime(context, hash, limits(this.env)));
  }
  async alarm(): Promise<void> {
    this.#store.cleanup(seconds());
    const next = this.#store.nextExpiry();
    if (next !== null) await this.#wake.at(Math.max(Date.now() + 1000, next * 1000));
  }
}
