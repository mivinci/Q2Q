import { DurableObject } from 'cloudflare:workers';
import { z } from 'zod';
import { environmentSchema, integer, metricsSchema } from '@q2q/punch-discovery';
import { fetchSnapshot } from './discovery.ts';
import { ProtocolError } from './protocol.ts';
import { Alarms, assertObject, checked, hasTable, limits, punchSchema, seconds, timeSchema, unavailable, within } from './state.ts';
import type { Bindings } from './state.ts';

export const DISCOVERY_NAME = 'punch-discovery:v1';
const snapshotSchema = z.object({
  expires_at: timeSchema,
  nodes: z.array(punchSchema.extend({ accepting_new_connections: z.boolean(), expires_at: timeSchema,
    identity_version: integer.positive().optional(), metrics: metricsSchema.nullable().optional() })).max(1024)
});
export type DirectorySnapshot = z.infer<typeof snapshotSchema>;

/** One persistent, expiring snapshot; all pages validate before atomic replacement. */
export class DiscoveryState extends DurableObject<Bindings> {
  readonly #wake: Alarms;
  #refreshing?: Promise<void>;
  #activeRpc?: Promise<unknown>;
  constructor(ctx: DurableObjectState, env: Bindings) { super(ctx, env); this.#wake = new Alarms(ctx.storage); }
  #ready(): boolean {
    if (!hasTable(this.ctx.storage, 'discovery_meta')) {
      if (hasTable(this.ctx.storage, 'discovery_snapshot')) unavailable();
      return false;
    }
    if (this.ctx.storage.sql.exec<{version:number}>('SELECT version FROM discovery_meta WHERE id=1').one().version !== 1) unavailable();
    return true;
  }
  #load(): DirectorySnapshot | null {
    assertObject(this.ctx.id, this.env.DISCOVERY, DISCOVERY_NAME);
    if (!this.#ready()) return null;
    const row = this.ctx.storage.sql.exec<{payload:string}>('SELECT payload FROM discovery_snapshot WHERE id=1').toArray()[0];
    if (!row) return null;
    const value = checked(snapshotSchema, JSON.parse(row.payload));
    return value.expires_at > seconds() ? value : null;
  }
  async snapshot(): Promise<DirectorySnapshot> {
    const current = this.#load();
    if (current) return current;
    await this.#refresh();
    const refreshed = this.#load();
    if (!refreshed) throw new ProtocolError('discovery_unavailable');
    return refreshed;
  }
  async publish(input: DirectorySnapshot): Promise<void> {
    assertObject(this.ctx.id, this.env.DISCOVERY, DISCOVERY_NAME);
    const value = checked(snapshotSchema, input), now = seconds();
    if (value.expires_at <= now || value.expires_at > now + 90 || value.nodes.some(node => node.expires_at <= now || value.expires_at > node.expires_at)
      || new Set(value.nodes.map(node => node.punch_id)).size !== value.nodes.length) throw new ProtocolError('bad_request');
    const payload = JSON.stringify(value);
    if (new TextEncoder().encode(payload).length > 1048576) throw new ProtocolError('bad_request');
    await this.#wake.at(value.expires_at * 1000);
    this.ctx.storage.transactionSync(() => {
      if (!this.#ready()) this.ctx.storage.sql.exec(`
        CREATE TABLE discovery_meta(id INTEGER PRIMARY KEY CHECK(id=1), version INTEGER NOT NULL);
        INSERT INTO discovery_meta VALUES(1,1);
        CREATE TABLE discovery_snapshot(id INTEGER PRIMARY KEY CHECK(id=1), payload TEXT NOT NULL);
      `);
      this.ctx.storage.sql.exec('CREATE TABLE IF NOT EXISTS discovery_identities(id TEXT PRIMARY KEY, version INTEGER NOT NULL, url TEXT NOT NULL, public_key TEXT NOT NULL)');
      for (const node of value.nodes) {
        if (node.identity_version === undefined) continue; // Minimal trusted fixtures from protocol/state tests.
        const previous = this.ctx.storage.sql.exec<{version:number; url:string; public_key:string}>('SELECT * FROM discovery_identities WHERE id=?', node.punch_id).toArray()[0];
        if (previous && (previous.version > node.identity_version || (previous.version === node.identity_version &&
          (previous.url !== node.url || previous.public_key !== node.public_key)))) throw new ProtocolError('discovery_unavailable');
        if (!previous && this.ctx.storage.sql.exec<{n:number}>('SELECT count(*) AS n FROM discovery_identities').one().n >= 4096) throw new ProtocolError('discovery_unavailable');
        this.ctx.storage.sql.exec('INSERT OR REPLACE INTO discovery_identities VALUES(?,?,?,?)', node.punch_id, node.identity_version, node.url, node.public_key);
      }
      this.ctx.storage.sql.exec('INSERT OR REPLACE INTO discovery_snapshot VALUES(1,?)', payload);
    });
  }
  #refresh(): Promise<void> {
    if (this.#refreshing) return this.#refreshing;
    const task = this.#doRefresh().finally(() => {if (this.#refreshing === task) this.#refreshing = undefined;});
    this.#refreshing = task;
    return task;
  }
  async #doRefresh(): Promise<void> {
    await this.#wake.at(Date.now() + 30000); // Persist recovery before any network wait.
    const retry = await this.ctx.storage.get<{failures:number; retry_at:number}>('discovery-retry');
    if (retry && retry.retry_at > Date.now()) {
      await this.#wake.at(retry.retry_at);
      throw new ProtocolError('discovery_unavailable');
    }
    try {
      const environment = environmentSchema.parse(this.env.REGISTRY_ENVIRONMENT);
      const binding = this.env.REGISTRY;
      if (!binding || this.#activeRpc) throw new ProtocolError('discovery_unavailable');
      const deadline = Date.now() + limits(this.env).REQUEST_TIMEOUT_MS;
      const snapshot = await fetchSnapshot(async request => {
        if (Date.now() >= deadline || this.#activeRpc) throw new ProtocolError('discovery_unavailable');
        const rpc = Promise.resolve(binding.getPunches(request));
        this.#activeRpc = rpc;
        void rpc.then(() => {if (this.#activeRpc === rpc) this.#activeRpc = undefined;}, () => {if (this.#activeRpc === rpc) this.#activeRpc = undefined;});
        return within(rpc, Math.max(1, deadline - Date.now()));
      }, environment);
      await this.publish(snapshot);
      await this.ctx.storage.delete('discovery-retry');
      await this.#wake.at(Math.min(Date.now() + 30000, Math.max(Date.now() + 1000, snapshot.expires_at * 1000 - 1000)));
    } catch {
      const failures = Math.min(6, (retry?.failures ?? 0) + 1);
      const retry_at = Date.now() + Math.min(30000, 1000 * 2 ** failures) + Math.floor(Math.random() * 250);
      await this.ctx.storage.put('discovery-retry', {failures, retry_at});
      await this.#wake.at(retry_at);
      throw new ProtocolError('discovery_unavailable');
    }
  }
  async alarm(): Promise<void> {
    try {await this.#refresh();} catch { /* Keep only still-valid cached data; retries are persisted. */ }
    if (this.#ready()) this.ctx.storage.sql.exec('DELETE FROM discovery_snapshot WHERE json_extract(payload,\'$.expires_at\')<=?', seconds());
  }
}
