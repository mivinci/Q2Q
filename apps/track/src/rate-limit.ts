import { DurableObject } from 'cloudflare:workers';
import { z } from 'zod';
import { Alarms, checked } from './state.ts';
import type { Bindings } from './state.ts';

/** One hashed client network per object; all entry replicas share the same buckets. */
export class RateLimiter extends DurableObject<Bindings> {
  readonly #wake: Alarms;
  constructor(ctx: DurableObjectState, env: Bindings) {super(ctx, env); this.#wake = new Alarms(ctx.storage);}
  async take(input: 'announce' | 'session'): Promise<boolean> {
    const kind = checked(z.enum(['announce', 'session']), input), now = Date.now();
    await this.#wake.at(now + 120000);
    return this.ctx.storage.transactionSync(() => {
      this.ctx.storage.sql.exec('CREATE TABLE IF NOT EXISTS buckets(kind TEXT PRIMARY KEY, tokens REAL NOT NULL, updated REAL NOT NULL)');
      const previous = this.ctx.storage.sql.exec<{tokens:number; updated:number}>('SELECT * FROM buckets WHERE kind=?', kind).toArray()[0];
      const capacity = kind === 'announce' ? 10 : 60;
      const tokens = Math.min(capacity, (previous?.tokens ?? capacity) + Math.max(0, now - (previous?.updated ?? now)) * capacity / 60000);
      const accepted = tokens >= 1;
      this.ctx.storage.sql.exec('INSERT OR REPLACE INTO buckets VALUES(?,?,?)', kind, tokens - Number(accepted), now);
      return accepted;
    });
  }
  async alarm(): Promise<void> {
    const recent = this.ctx.storage.sql.exec<{updated:number}>('SELECT max(updated) AS updated FROM buckets').one().updated;
    if (recent + 120000 > Date.now()) await this.#wake.at(recent + 120000);
    else await this.ctx.storage.deleteAll();
  }
}
