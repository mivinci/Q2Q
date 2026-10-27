// Test-only RPC adapters. They are never exported by src/index.ts or deployed.
import { DurableObject } from 'cloudflare:workers';
import vectors from '../../../tests/fixtures/track/v1.json' with { type: 'json' };
import { PeerState, PeerStore } from '../src/peer-state.ts';
import { FileIndex, FileStore } from '../src/file-index.ts';
import { DiscoveryState, DISCOVERY_NAME } from '../src/directory.ts';
import app from '../src/index.ts';
import { instrument } from './benchmark-meter.ts';
export { RateLimiter } from '../src/rate-limit.ts';
import { base64, canonicalTicket, decodeBase64, peerId, sessionMac, stringifyWire, turnCredential } from '../src/protocol.ts';
import { Alarms, blob, limits, seconds } from '../src/state.ts';
import type { Bindings, IndexEvent, QueryContext } from '../src/state.ts';
import type { NewSession } from '../src/peer-state.ts';

const hash = (n: number) => `sha256:${n.toString(16).padStart(2, '0').repeat(32)}`;
const fixturePeer = JSON.parse(vectors.tickets[0]!.body_json).iss as string;
const punch = { punch_id: 'fixture-punch', url: 'quic://punch.invalid:443', public_key: base64(new Uint8Array(32).fill(9)) };
app.get('/contract-error', async context => {
  await context.env.PEERS.getByName(`peer:${fixturePeer}`).authenticate({ peer_id: fixturePeer, seq: '1', timestamp: String(seconds()), mac: '' }, false);
  return context.json({ unexpected: true });
});
app.get('/contract-unavailable', () => { throw new Error('no_available_punch'); });
function equal(actual: unknown, expected: unknown): void {
  const stable = (value: unknown): unknown => Array.isArray(value) ? value.map(stable)
    : value !== null && typeof value === 'object' ? Object.fromEntries(Object.entries(value).sort(([a], [b]) => a.localeCompare(b)).map(([key, item]) => [key, stable(item)])) : value;
  if (JSON.stringify(stable(actual)) !== JSON.stringify(stable(expected))) throw new Error(`expected ${JSON.stringify(expected)}, received ${JSON.stringify(actual)}`);
}
async function error(action: () => unknown | Promise<unknown>, expected = 'storage_unavailable'): Promise<void> {
  try { await action(); } catch (cause) {
    if (cause instanceof Error && cause.message.includes(expected)) return;
    throw cause;
  }
  throw new Error(`expected ${expected}`);
}
function tables(storage: DurableObjectStorage): number {
  return storage.sql.exec<{n:number}>("SELECT count(*) AS n FROM sqlite_master WHERE type='table' AND name IN ('peer_meta','session','holdings','outbox','file_meta','members','discovery_meta','discovery_snapshot')").one().n;
}
const record = (key: number, expiry: number): NewSession => ({ peer_id: fixturePeer, public_key: base64(new Uint8Array(32).fill(7)),
  session_key: base64(new Uint8Array(32).fill(key)), expires_at: expiry, punch });

export class StoreHarness extends DurableObject<Bindings> {
  // Scheduler scenarios inspect and clear timers before they fire.
  async alarm(): Promise<void> {}
  async trace(index: number): Promise<unknown[]> {
    const store = new PeerStore(this.ctx.storage, limits(this.env));
    const outputs: unknown[] = [];
    for (const step of vectors.state_traces[index]!.steps) {
      const value = step as Record<string, unknown>;
      const n = (key: string) => Number(value[key]);
      const list = (key: string) => (value[key] as number[]).map(hash);
      switch (value.op) {
        case 'announce': store.announce(record(n('key'), n('expiry')), list('add'), list('del'), list('complete')); outputs.push('ok'); break;
        case 'advance': {
          try { store.advance(record(n('key'), 0).session_key, BigInt(value.seq as string), value.renew === null ? null : n('renew'), n('now')); outputs.push('ok'); }
          catch (cause) { outputs.push((cause as Error).message); }
          break;
        }
        case 'query': {
          const holding = store.holding(hash(n('hash')), n('now'));
          outputs.push(holding ? [{ peer_id: holding.peer_id, complete: holding.complete }] : []); break;
        }
        case 'online': {
          const session = store.session();
          outputs.push(session && session.expires_at > n('now') ? { seq: session.last_seq, expiry: session.expires_at.toString() } : null); break;
        }
      }
    }
    return outputs;
  }
  async scanBudgetForTest() {
    const file = new FileStore(this.ctx.storage, limits(this.env));
    for (let index = 0; index < 1000; index++) {
      const bytes = new Uint8Array(32); new DataView(bytes.buffer).setUint32(0, index, true);
      file.apply({peer_id: await peerId(bytes), info_hash: hash(80), revision: '1', deleted: false, complete: true, expires_at: 1000});
    }
    const meter = instrument(this.ctx.storage);
    const page = file.page(null, 64, 100);
    return {rows_read: meter.counts.rows_read, count: page.length, ordered: page.every((row, index) => index === 0 || row.peer_id > page[index - 1]!.peer_id)};
  }
  async scenario(name: string): Promise<void> {
    const config = limits(this.env);
    const store = new PeerStore(this.ctx.storage, config);
    const event = (revision: string, deleted = false, complete = false): IndexEvent => ({ peer_id: fixturePeer, info_hash: hash(1), revision, deleted, complete, expires_at: 150 });
    if (name === 'alarm-reconcile') {
      const wake = new Alarms(this.ctx.storage), early = Date.now() + 1000, late = Date.now() + 60000;
      let committed = false;
      const writer = (async () => {await wake.at(early); committed = true;})();
      await Promise.all([writer, wake.reconcile(() => committed ? early : late)]);
      equal(await this.ctx.storage.getAlarm(), early);
      await wake.reconcile(() => late); equal(await this.ctx.storage.getAlarm(), late);
      await wake.reconcile(() => null); equal(await this.ctx.storage.getAlarm(), null);
    } else if (name === 'empty-reads') {
      equal(store.session(), null); equal(store.holding(hash(1), 100), null); equal(store.pending(), []);
      const file = new FileStore(this.ctx.storage, config);
      equal(file.page(null, 64, 100), []); equal(tables(this.ctx.storage), 0);
    } else if (name === 'rollback') {
      const limited = new PeerStore(this.ctx.storage, { ...config, MAX_PEER_HOLDINGS: 1 });
      limited.announce(record(7, 300), [hash(1)], [], []);
      const before = limited.session(), pending = limited.pending();
      await error(() => limited.announce(record(8, 400), [hash(2)], [], []));
      equal(limited.session(), before); equal(limited.pending(), pending); equal(limited.holding(hash(2), 100), null);
    } else if (name === 'initial-quota-rollback') {
      const limited = new PeerStore(this.ctx.storage, { ...config, MAX_OUTBOX_BYTES: 1 });
      await error(() => limited.announce(record(7, 300), [hash(1)], [], []));
      equal(tables(this.ctx.storage), 0); equal(limited.session(), null);
    } else if (name === 'outbox-count-quota') {
      const limited = new PeerStore(this.ctx.storage, { ...config, MAX_OUTBOX_EVENTS: 1 });
      limited.announce(record(7, 300), [hash(1)], [], []);
      const before = limited.session();
      await error(() => limited.announce(record(8, 400), [hash(2)], [], []));
      equal(limited.session(), before); equal(limited.pendingCount(), 1);
    } else if (name === 'ack-race') {
      store.announce(record(7, 300), [hash(1)], [], []); const older = store.pending()[0]!;
      store.announce(record(8, 400), [hash(1)], [], [hash(1)]); const newer = store.pending()[0]!;
      store.ack(older); equal(store.pending(), [newer]); store.ack(newer); equal(store.pendingCount(), 0);
    } else if (name === 'unsigned-sql') {
      store.announce(record(7, 300), [], [], []);
      const values = [1n, 9007199254740993n, 9223372036854775808n, 18446744073709551615n];
      this.ctx.storage.sql.exec('CREATE TABLE numbers(seq BLOB PRIMARY KEY)');
      for (const seq of [...values].reverse()) this.ctx.storage.sql.exec('INSERT INTO numbers VALUES(?)', blob(seq));
      equal(this.ctx.storage.sql.exec<{n:string}>('SELECT hex(seq) AS n FROM numbers ORDER BY seq').toArray().map(row => BigInt(`0x${row.n}`).toString()), values.map(String));
      for (const seq of values) { store.advance(record(7, 0).session_key, seq, null, 100); equal(store.session()!.last_seq, seq.toString()); }
      await error(() => store.advance(record(7, 0).session_key, values[3]!, null, 100), 'bad_mac');
    } else if (name === 'version-exhaustion') {
      store.announce(record(7, 300), [hash(1)], [], []); const before = store.session();
      this.ctx.storage.sql.exec('UPDATE peer_meta SET counter=?', blob((1n << 64n) - 1n));
      await error(() => store.announce(record(8, 400), [hash(2)], [], [])); equal(store.session(), before);
    } else if (name === 'expiry-keeps-counter') {
      store.announce(record(7, 150), [hash(1)], [], []); const older = store.pending()[0]!;
      store.cleanup(150); equal(store.session(), null);
      store.announce(record(8, 300), [hash(1)], [], []); const newer = store.pending()[0]!;
      if (BigInt(newer.revision) <= BigInt(older.revision)) throw new Error('revision was reset');
    } else if (name === 'file-ordering') {
      const file = new FileStore(this.ctx.storage, config);
      equal(file.apply(event('1')), 'applied'); equal(file.apply(event('1')), 'duplicate');
      equal(file.apply(event('2', true)), 'applied'); equal(file.apply(event('1')), 'stale'); equal(file.page(null, 64, 100), []);
      await error(() => file.apply(event('2', false))); equal(file.page(null, 64, 100), []);
      equal(file.apply(event('3', false, true)), 'applied'); equal(file.page(null, 64, 100), [{ peer_id: fixturePeer, revision: '3' }]);
    } else if (name === 'file-expiry-idempotence') {
      const file = new FileStore(this.ctx.storage, config);
      file.apply(event('1')); file.cleanup(150); equal(file.apply(event('1')), 'duplicate');
      equal(file.page(null, 64, 150), []); equal(file.apply(event('0')), 'stale');
    } else if (name === 'file-quota') {
      const file = new FileStore(this.ctx.storage, { ...config, MAX_FILE_PEERS: 1 });
      file.apply(event('1'));
      const other = { ...event('2'), peer_id: vectors.identities[0]!.peer_id };
      await error(() => file.apply(other)); equal(file.page(null, 64, 100), [{ peer_id: fixturePeer, revision: '1' }]);
    } else if (name === 'future-schema') {
      store.announce(record(7, 300), [], [], []);
      this.ctx.storage.sql.exec('UPDATE peer_meta SET version=2');
      await error(() => store.session()); await error(() => store.announce(record(8, 400), [], [], []));
    } else if (name === 'file-future-schema') {
      const file = new FileStore(this.ctx.storage, config); file.apply(event('1'));
      this.ctx.storage.sql.exec('UPDATE file_meta SET version=2');
      await error(() => file.page(null, 64, 100)); await error(() => file.apply(event('2')));
    } else throw new Error(`unknown scenario ${name}`);
  }
}

export class TestPeer extends PeerState {
  #readMode = 'ok';
  #reads: (() => void)[] = [];
  async settleAlarmForTest() {await this.ctx.storage.deleteAlarm(); await super.alarm(); return this.ctx.storage.getAlarm();}
  seedForTest(record: NewSession, hash: string) {
    const store = new PeerStore(this.ctx.storage, limits(this.env));
    store.announce(record, [hash], [], [hash]);
    return store.pending()[0]!;
  }
  readModeForTest(mode: string) {this.#readMode = mode;}
  releaseReadsForTest() {for (const resolve of this.#reads.splice(0)) resolve();}
  override async candidate(hash: string, revision: string, context?: QueryContext) {
    if (this.#readMode === 'fail') throw new Error('storage_unavailable');
    if (this.#readMode === 'gate') await new Promise<void>(resolve => this.#reads.push(resolve));
    return super.candidate(hash, revision, context);
  }
  removeForTest(hash: string) {
    const store = new PeerStore(this.ctx.storage, limits(this.env));
    const session = store.session();
    if (!session) throw new Error('missing test session');
    store.announce({...session, expires_at: seconds() + 300}, [], [hash], []);
  }
  expireForTest() {this.ctx.storage.sql.exec('UPDATE session SET expires_at=0');}

  async inspectForTest() { const store = new PeerStore(this.ctx.storage, limits(this.env)); return { session: store.session(), pending: store.pending(), alarm: await this.ctx.storage.getAlarm(), tables: tables(this.ctx.storage) }; }
  retryForTest() { return this.alarm(); }
}
export class FaultFile extends FileIndex {
  private gate: (() => void)[] = [];
  async modeForTest(mode: string) { await this.ctx.storage.put('fault-mode', mode); }
  waitingForTest() { return this.gate.length; }
  releaseForTest() { for (const resolve of this.gate.splice(0)) resolve(); }
  override async page(hash: string, after: string | null, limit: number, context?: QueryContext) {
    const mode = await this.ctx.storage.get<string>('fault-mode');
    if (mode === 'read-fail') throw new Error('storage_unavailable');
    return super.page(hash, after, limit, context);
  }
  override async apply(event: IndexEvent) {
    const mode = await this.ctx.storage.get<string>('fault-mode');
    if (mode === 'fail') throw new Error('storage_unavailable');
    if (mode === 'gate') await new Promise<void>(resolve => this.gate.push(resolve));
    const result = await super.apply(event);
    if (mode === 'lost-ack') throw new Error('storage_unavailable');
    return result;
  }
}
export class TestDirectory extends DiscoveryState {
  async refreshForTest() {await this.ctx.storage.delete('discovery-retry'); await this.alarm();}
  expireForTest() {this.ctx.storage.sql.exec("UPDATE discovery_snapshot SET payload=json_set(payload, '$.expires_at', 0)");}
  async statusForTest() {return {alarm: await this.ctx.storage.getAlarm(), retry: await this.ctx.storage.get('discovery-retry')};}

  futureVersionForTest() { this.ctx.storage.sql.exec('UPDATE discovery_meta SET version=2'); }
}
interface TestBindings extends Bindings {
  PEERS: DurableObjectNamespace<TestPeer>;
  FILES: DurableObjectNamespace<FaultFile>;
  HARNESS: DurableObjectNamespace<StoreHarness>;
  DISCOVERY: DurableObjectNamespace<TestDirectory>;
}
async function identity() {
  const keys = await crypto.subtle.generateKey('Ed25519', true, ['sign', 'verify']) as CryptoKeyPair;
  const pub = base64(new Uint8Array(await crypto.subtle.exportKey('raw', keys.publicKey)));
  const id = await peerId(new Uint8Array(await crypto.subtle.exportKey('raw', keys.publicKey)));
  return { keys, pub, id };
}
async function request(who: Awaited<ReturnType<typeof identity>>, add: string[] = [], del: string[] = []) {
  const now = BigInt(seconds());
  const ticket = { v: 2n, iss: who.id, pub: who.pub, iat: now, exp: now + 60n, aud: 'track', sig: '' };
  ticket.sig = base64(new Uint8Array(await crypto.subtle.sign('Ed25519', who.keys.privateKey, Uint8Array.from(canonicalTicket(ticket)).buffer)));
  return { peer_id: who.id, token: stringifyWire(ticket), add, del, complete: add, ttl: '300', punch };
}
async function until(check: () => Promise<boolean>, timeout = 4000): Promise<void> {
  const end = Date.now() + timeout;
  while (!await check()) { if (Date.now() >= end) throw new Error('condition timed out'); await new Promise(resolve => setTimeout(resolve, 10)); }
}
async function actorChecks(env: TestBindings): Promise<string[]> {
  const passed: string[] = [];
  for (let i = 0; i < vectors.state_traces.length; i++) {
    equal(await env.HARNESS.getByName(`trace:${i}`).trace(i), vectors.state_traces[i]!.outputs); passed.push(`shared-trace:${i}`);
  }
  for (const scenario of ['alarm-reconcile','empty-reads','rollback','initial-quota-rollback','outbox-count-quota','ack-race','unsigned-sql','version-exhaustion',
    'expiry-keeps-counter','file-ordering','file-expiry-idempotence','file-quota','future-schema','file-future-schema']) {
    await env.HARNESS.getByName(scenario).scenario(scenario); passed.push(scenario);
  }
  {
    const who = await identity(), peer = env.PEERS.getByName(`peer:${who.id}`), hash1 = hash(31);
    const file = env.FILES.getByName(`file:${hash1}`);
    const joined = await peer.announce(await request(who, [hash1]));
    let hidden = false;
    try { await (peer as unknown as {flush():Promise<void>}).flush(); } catch { hidden = true; }
    equal(hidden, true);
    const page = await file.page(hash1, null, 10); equal(page.length, 1);
    const candidate = await peer.candidate(hash1, page[0]!.revision); equal(candidate?.peer_id, who.id);
    if (candidate && 'session_key' in candidate) throw new Error('candidate leaked session key');
    const now = BigInt(seconds());
    const mac = await sessionMac(Uint8Array.from(atob(joined.session_key), c => c.charCodeAt(0)), who.id, 1n, now);
    const auth = { peer_id: who.id, seq: '1', timestamp: now.toString(), mac };
    const attempts = await Promise.allSettled([peer.authenticate(auth, false), peer.authenticate(auth, true)]);
    equal(attempts.filter(value => value.status === 'fulfilled').length, 1);
    equal((await peer.inspectForTest()).session!.last_seq, '1');
    await peer.announce(await request(who, [], [hash1])); equal(await file.page(hash1, null, 10), []);
    await error(() => peer.authenticate({ ...auth, seq: '2' }, false), 'bad_mac');
    passed.push('real-auth-concurrent-sequence-and-delete');
  }
  {
    const who = await identity(), peer = env.PEERS.getByName(`peer:${who.id}`), goodHash = hash(32), badHash = hash(33);
    const bad = env.FILES.getByName(`file:${badHash}`);
    await bad.modeForTest('fail');
    await error(async () => peer.announce(await request(who, [goodHash, badHash])));
    equal((await peer.inspectForTest()).pending.length, 1);
    const retained = await peer.announce(await request(who));
    equal((await peer.inspectForTest()).session!.session_key, retained.session_key);
    equal((await peer.inspectForTest()).pending.length, 1);
    equal((await env.FILES.getByName(`file:${goodHash}`).page(goodHash, null, 10)).length, 1);
    if ((await peer.inspectForTest()).alarm === null) throw new Error('pending event lacks durable alarm');
    await bad.modeForTest('ok');
    // Real scheduled alarm, not only a manual call of the handler.
    await until(async () => (await peer.inspectForTest()).pending.length === 0);
    equal((await bad.page(badHash, null, 10)).length, 1); passed.push('partial-delivery-automatic-alarm-recovery');
  }
  {
    const who = await identity(), peer = env.PEERS.getByName(`peer:${who.id}`), value = hash(34), file = env.FILES.getByName(`file:${value}`);
    await file.modeForTest('lost-ack'); await error(async () => peer.announce(await request(who, [value])));
    equal((await file.page(value, null, 10)).length, 1); equal((await peer.inspectForTest()).pending.length, 1);
    await file.modeForTest('ok'); await peer.retryForTest(); equal((await peer.inspectForTest()).pending.length, 0);
    passed.push('applied-write-lost-ack-idempotent-retry');
  }
  {
    const who = await identity(), peer = env.PEERS.getByName(`peer:${who.id}`), value = hash(36), file = env.FILES.getByName(`file:${value}`);
    await file.modeForTest('gate');
    try {
      await error(async () => peer.announce(await request(who, [value])));
      equal((await peer.inspectForTest()).pending.length, 1);
      await file.modeForTest('ok'); await file.releaseForTest();
      await until(async () => { await peer.retryForTest(); return (await peer.inspectForTest()).pending.length === 0; });
      equal((await file.page(value, null, 10)).length, 1);
      passed.push('rpc-deadline-and-late-delivery-recovery');
    } finally { await file.releaseForTest(); await file.modeForTest('ok'); }
  }
  {
    const who = await identity(), peer = env.PEERS.getByName(`peer:${who.id}`), value = hash(35), file = env.FILES.getByName(`file:${value}`);
    await file.modeForTest('gate');
    const first = peer.announce(await request(who, [value])).then(() => 'unexpected-success', cause => (cause as Error).message);
    try {
      await until(async () => await file.waitingForTest() > 0);
      const generation = (await peer.inspectForTest()).session!.generation;
      const second = peer.announce(await request(who));
      await until(async () => (await peer.inspectForTest()).session!.generation !== generation);
      await file.releaseForTest();
      const joined = await second;
      if (!(await first).includes('storage_unavailable')) throw new Error('superseded announce returned a key');
      equal((await peer.inspectForTest()).session!.session_key, joined.session_key);
      passed.push('reannounce-during-delivery');
    } finally { await file.releaseForTest(); await file.modeForTest('ok'); }
  }
  {
    const empty = await identity(), peer = env.PEERS.getByName(`peer:${empty.id}`);
    await error(() => peer.authenticate({ peer_id: empty.id, seq: '1', timestamp: String(seconds()), mac: '' }, true), 'unknown_session');
    equal((await peer.inspectForTest()).tables, 0);
    const body = await request(empty); body.token = body.token.replace('"sig":"', '"sig":"!');
    await error(() => peer.announce(body), 'malformed_ticket'); equal((await peer.inspectForTest()).tables, 0);
    await error(async () => env.PEERS.getByName('peer:wrong').announce(await request(empty)), 'bad_request');
    passed.push('unauthenticated-and-wrong-object-do-not-write');
  }
  {
    const directory = env.DISCOVERY.getByName(DISCOVERY_NAME), now = seconds();
    await error(() => directory.snapshot(), 'discovery_unavailable');
    const snapshot = { expires_at: now + 30, nodes: [{ ...punch, accepting_new_connections: true, expires_at: now + 60 }] };
    await directory.publish(snapshot); equal(await directory.snapshot(), snapshot);
    await error(() => directory.publish({ ...snapshot, expires_at: now + 3600 }), 'bad_request');
    await directory.futureVersionForTest();
    await error(() => directory.snapshot()); await error(() => directory.publish(snapshot));
    passed.push('bounded-persistent-discovery-snapshot');
  }
  return passed;
}

export default {
  async fetch(incoming: Request, env: TestBindings): Promise<Response> {
    const path = new URL(incoming.url).pathname;
    if (path === '/performance-checks') {
      const scan = await env.HARNESS.getByName('file-scan-budget').scanBudgetForTest();
      const who = await identity(), peer = env.PEERS.getByName(`peer:${who.id}`);
      const announced = await peer.announce(await request(who));
      const announcedAlarm = (await peer.inspectForTest()).alarm;
      const announceSchedulesExpiry = announcedAlarm !== null && announcedAlarm > Date.now() + 200000;
      const before = await peer.settleAlarmForTest(), timestamp = BigInt(seconds());
      await peer.authenticate({peer_id: who.id, seq: '1', timestamp: String(timestamp), mac: await sessionMac(decodeBase64(announced.session_key, 32), who.id, 1n, timestamp)}, true);
      const after = (await peer.inspectForTest()).alarm;
      return Response.json({scan, announce_schedules_expiry_after_ack: announceSchedulesExpiry, heartbeat_preserves_earlier_expiry_alarm: before === after});
    }
    if (path === '/discovery-test') {
      const directory = env.DISCOVERY.getByName(DISCOVERY_NAME);
      try {
        const operation = new URL(incoming.url).searchParams.get('operation');
        if (operation === 'refresh') {await directory.refreshForTest(); return Response.json({refreshed: true});}
        if (operation === 'expire') {await directory.expireForTest(); return Response.json({expired: true});}
        if (operation === 'status') return Response.json(await directory.statusForTest());
        if (operation === 'write') {
          const binding = env.REGISTRY as unknown as {report(): Promise<unknown>};
          await binding.report(); return Response.json({unexpected: true});
        }
        return Response.json(await directory.snapshot());
      } catch {return Response.json({code: 'discovery_unavailable'}, {status: 503});}
    }
    if (path === '/query-checks') {
      try {
        const who = await identity(), requester = env.PEERS.getByName(`peer:${who.id}`), content = hash(73);
        const auth = await requester.announce(await request(who));
        await env.DISCOVERY.getByName(DISCOVERY_NAME).publish({expires_at: seconds() + 60, nodes: [{...punch, accepting_new_connections: true, expires_at: seconds() + 90}]});
        const ids = (await Promise.all(Array.from({length: 66}, (_, i) => peerId(new Uint8Array(32).fill(i))))).sort();
        const file = env.FILES.getByName(`file:${content}`);
        for (const id of ids.slice(1, 65)) await file.apply({peer_id: id, info_hash: content, revision: '1', deleted: false, complete: true, expires_at: seconds() + 300});
        const target = env.PEERS.getByName(`peer:${ids[65]}`);
        const event = await target.seedForTest({peer_id: ids[65]!, public_key: punch.public_key, session_key: base64(new Uint8Array(32)), punch, expires_at: seconds() + 300}, content);
        await file.apply(event);
        let seq = 0n;
        async function query(cursor = ids[0]!, replay = false) {
          if (!replay) seq++;
          const timestamp = BigInt(seconds());
          const params = new URLSearchParams({peer_id: who.id, seq: String(seq), timestamp: String(timestamp), cursor, limit: '10',
            mac: await sessionMac(decodeBase64(auth.session_key, 32), who.id, seq, timestamp)});
          return app.request(`https://track.invalid/files/${content}/peers?${params}`, {}, env);
        }
        const short = await query(); equal(short.status, 200);
        const first = await short.json() as {peers:unknown[]; next_cursor:string};
        equal(first.peers, []); if (!first.next_cursor) throw new Error('budget lost continuation');
        const continued = await query(first.next_cursor); equal(continued.status, 200);
        const next = await continued.json() as {peers:{peer_id:string}[]; next_cursor:string|null};
        equal(next.peers.map(peer => peer.peer_id), [ids[65]]); equal(next.next_cursor, null);
        await target.readModeForTest('fail'); equal((await query(ids[64]!)).status, 503);
        equal((await query(ids[64]!, true)).status, 401); // Failed query still consumed seq.
        await target.readModeForTest('gate'); equal((await query(ids[64]!)).status, 503);
        await target.releaseReadsForTest(); await target.readModeForTest('ok');
        equal((await query(ids[64]!)).status, 200);
        await file.modeForTest('read-fail'); equal((await query(ids[64]!)).status, 503);
        await file.modeForTest('ok');
        // A newer index event cannot invent a matching authoritative holding.
        await file.apply({...event, revision: String(BigInt(event.revision) + 1n)});
        const mismatched = await query(ids[64]!); equal(mismatched.status, 200);
        equal((await mismatched.json() as {peers:unknown[]}).peers, []);
        // Restore a matching version before testing expiry independently.
        const restored = await target.seedForTest({peer_id: ids[65]!, public_key: punch.public_key, session_key: base64(new Uint8Array(32)), punch, expires_at: seconds() + 300}, content);
        await file.apply(restored);
        const visible = await query(ids[64]!); equal(visible.status, 200);
        equal((await visible.json() as {peers:unknown[]}).peers.length, 1);
        // Expired authoritative sessions must also hide live-looking index rows.
        await target.expireForTest();
        const expired = await query(ids[64]!); equal(expired.status, 200);
        equal((await expired.json() as {peers:unknown[]}).peers, []);
        const fresh = await target.seedForTest({peer_id: ids[65]!, public_key: punch.public_key, session_key: base64(new Uint8Array(32)), punch, expires_at: seconds() + 300}, content);
        await file.apply(fresh); await target.removeForTest(content); // Tombstone deliberately not delivered.
        const delayedDelete = await query(ids[64]!); equal(delayedDelete.status, 200);
        equal((await delayedDelete.json() as {peers:unknown[]}).peers, []);
        seq++;
        const timestamp = BigInt(seconds());
        const authenticated = await requester.authorizeQuery({peer_id: who.id, seq: String(seq), timestamp: String(timestamp), mac: await sessionMac(decodeBase64(auth.session_key, 32), who.id, seq, timestamp)}, content, Date.now() + 400);
        equal(Object.keys(authenticated).sort(), ['deadline', 'info_hash', 'now', 'requester']);
        await error(() => file.page(content, null, 10, {...authenticated, info_hash: hash(74)}), 'bad_request');
        await error(() => target.candidate(content, fresh.revision, {...authenticated, deadline: Date.now() - 1}));
        return Response.json({passed: 12});
      } catch (error) {return Response.json({error: String(error), stack: (error as Error).stack}, {status: 500});}
    }
    if (path === '/sdk-config') {
      const node = await incoming.json() as typeof punch;
      await env.DISCOVERY.getByName(DISCOVERY_NAME).publish({expires_at: seconds() + 90, nodes: [{...node, accepting_new_connections: true, expires_at: seconds() + 90}]});
      return Response.json({configured: true});
    }
    if (path === '/sdk-expire') {
      const {peer_id} = await incoming.json() as {peer_id:string};
      await env.PEERS.getByName(`peer:${peer_id}`).expireForTest();
      return Response.json({expired: true});
    }
    if (path === '/http-checks') {
      try {
        let passed = 0;
        for (const scenario of vectors.http) {
          const response = await app.request('https://track.invalid' + scenario.uri, {
            method: scenario.method, headers: {'Content-Type': 'application/json'},
            ...(scenario.method === 'GET' ? {} : {body: scenario.body})
          }, env);
          equal(response.status, scenario.status);
          if (scenario.error) equal((await response.json() as {code:string}).code, scenario.error);
          passed++;
        }
        await env.DISCOVERY.getByName(DISCOVERY_NAME).publish({expires_at: seconds() + 60, nodes: [{...punch, accepting_new_connections: true, expires_at: seconds() + 90}]});
        const publisher = await identity(), reader = await identity(), content = hash(72);
        async function announce(who: Awaited<ReturnType<typeof identity>>, add: string[]) {
          const value = await request(who, add);
          const response = await app.request(`https://track.invalid/peers/${who.id}/files`, {method: 'PUT', headers: {'Content-Type': 'application/json'},
            body: `{"token":${value.token},"ttl":300,"add":${JSON.stringify(add)},"complete":${JSON.stringify(add)}}`}, env);
          equal(response.status, 200);
          const result = await response.json() as {session_key:string; punch:unknown; heartbeat_interval:number; turn:{credential:string; username:string}};
          equal((result.punch as {punch_id:string}).punch_id, punch.punch_id); equal(result.heartbeat_interval, 30);
          equal(result.turn.credential, await turnCredential(env.TURN_SECRET!, result.turn.username));
          return decodeBase64(result.session_key, 32);
        }
        await announce(publisher, [content]);
        const key = await announce(reader, []);
        const timestamp = BigInt(seconds());
        const mac = await sessionMac(key, reader.id, 1n, timestamp);
        const heartbeat = () => app.request(`https://track.invalid/peers/${reader.id}/heartbeat`, {method: 'POST', headers: {'Content-Type': 'application/json'}, body: stringifyWire({seq: 1n, timestamp, mac})}, env);
        equal((await heartbeat()).status, 200); equal((await heartbeat()).status, 401);
        const params = new URLSearchParams({peer_id: reader.id, seq: '2', timestamp: String(timestamp), mac: await sessionMac(key, reader.id, 2n, timestamp)});
        const queried = await app.request(`https://track.invalid/files/${content}/peers?${params}`, {}, env);
        equal(queried.status, 200);
        const result = await queried.json() as {peers:{peer_id:string}[]; next_cursor:string|null};
        equal(result.peers.map(peer => peer.peer_id), [publisher.id]); equal(result.next_cursor, null);
        await env.DISCOVERY.getByName(DISCOVERY_NAME).publish({expires_at: seconds() + 60, nodes: [{...punch, url: 'quic://changed.invalid:443', accepting_new_connections: true, expires_at: seconds() + 90}]});
        const moved = await app.request(`https://track.invalid/peers/${reader.id}/heartbeat`, {method: 'POST', headers: {'Content-Type': 'application/json'},
          body: stringifyWire({seq: 3n, timestamp, mac: await sessionMac(key, reader.id, 3n, timestamp)})}, env);
        equal(moved.status, 401);
        const recovered = await announce(reader, []);
        if (base64(recovered) === base64(key)) throw new Error('reannounce did not rotate key');
        const maximum = (1n << 64n) - 1n;
        const fullWidth = await app.request(`https://track.invalid/peers/${reader.id}/heartbeat`, {method: 'POST', headers: {'Content-Type': 'application/json'},
          body: stringifyWire({seq: maximum, timestamp, mac: await sessionMac(recovered, reader.id, maximum, timestamp)})}, env);
        equal(fullWidth.status, 200);
        const overflow = await app.request(`https://track.invalid/peers/${reader.id}/heartbeat`, {method: 'POST', headers: {'Content-Type': 'application/json'},
          body: `{"seq":18446744073709551616,"timestamp":${timestamp},"mac":""}`}, env);
        equal(overflow.status, 422);
        return Response.json({passed: passed + 1});
      } catch (error) {return Response.json({error: String(error), stack: (error as Error).stack}, {status: 500});}
    }
    if (path.startsWith('/peers/') || path.startsWith('/files/')) return app.fetch(incoming, env);
    if (path === '/health' || path === '/contract-error' || path === '/contract-unavailable') return app.fetch(incoming, env);
    if (path === '/inspect') {
      const input = await incoming.json() as {peer_id:string};
      return Response.json(await env.PEERS.getByName(`peer:${input.peer_id}`).inspectForTest());
    }
    if (path === '/checks') {
      try { return Response.json({ passed: await actorChecks(env) }); }
      catch (cause) { return Response.json({ error: String(cause), stack: (cause as Error).stack }, { status: 500 }); }
    }
    if (path === '/setup') {
      const who = await identity(), peer = env.PEERS.getByName(`peer:${who.id}`), value = hash(40), file = env.FILES.getByName(`file:${value}`);
      await file.modeForTest('fail'); await error(async () => peer.announce(await request(who, [value])));
      const state = await peer.inspectForTest();
      const tombstone: IndexEvent = { peer_id: who.id, info_hash: hash(41), revision: '20', deleted: true, complete: false, expires_at: seconds() + 300 };
      await env.FILES.getByName(`file:${tombstone.info_hash}`).apply(tombstone);
      return Response.json({ peer_id: who.id, value, session_key: state.session!.session_key, generation: state.session!.generation, tombstone });
    }
    if (path === '/restore') {
      const saved = await incoming.json() as {peer_id:string; value:string; session_key:string; generation:string; tombstone:IndexEvent};
      const peer = env.PEERS.getByName(`peer:${saved.peer_id}`), state = await peer.inspectForTest();
      equal(state.session!.session_key, saved.session_key); equal(state.session!.generation, saved.generation); equal(state.pending.length, 1);
      const file = env.FILES.getByName(`file:${saved.value}`);
      await file.modeForTest('ok'); await peer.retryForTest(); equal((await peer.inspectForTest()).pending.length, 0);
      const candidate = (await file.page(saved.value, null, 10))[0]!;
      equal((await peer.candidate(saved.value, candidate.revision))?.peer_id, saved.peer_id);
      equal(await env.FILES.getByName(`file:${saved.tombstone.info_hash}`).apply({ ...saved.tombstone, revision: '19', deleted: false }), 'stale');
      return Response.json({ restored: true });
    }
    return new Response(null, { status: 404 });
  }
};
