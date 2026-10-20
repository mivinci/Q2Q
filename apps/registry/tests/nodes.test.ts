import assert from 'node:assert/strict';
import test from 'node:test';
import { DatabaseSync } from 'node:sqlite';
import { readFileSync } from 'node:fs';
import { exportJWK, generateKeyPair, SignJWT } from 'jose';
import { app } from '../src/index.ts';

const pair = await generateKeyPair('EdDSA', { crv: 'Ed25519' });
const jwks = JSON.stringify({ keys: [{ ...await exportJWK(pair.publicKey), kid: 'test' }] });
const metrics = { quic_connections: 2, quic_connection_capacity: 1024, websocket_connections: 1,
  websocket_connection_capacity: 1024, live_bindings: 2, signalling_sessions: 1, signalling_session_capacity: null };
const reportBody = { version: '0.4.0', region: 'hk', accepting_new_connections: true, metrics };

// Test response expectations; production responses are validated with Zod.
type TestNode = {
  punch_id: string; public_key: string; accepting_new_connections: boolean; online: boolean;
  metrics: { signalling_session_capacity: number | null };
};
type TestJson = { nodes: TestNode[]; next_cursor: string | null; code: string; expires_at: number };
type TestResponse = Omit<Response, 'json'> & { json(): Promise<TestJson> };

function fixture(t: { after: (fn: () => void) => void }) {
  const sqlite = new DatabaseSync(':memory:');
  t.after(() => sqlite.close());
  for (const name of ['0001_nodes.sql', '0002_report_limits.sql']) {
    sqlite.exec(readFileSync(new URL(`../migrations/${name}`, import.meta.url), 'utf8'));
  }
  let databaseTime = Math.floor(Date.now() / 1000);
  sqlite.function('unixepoch', () => databaseTime);
  // Execute production SQL against SQLite; only adapt the D1 transport API.
  const db = { prepare(sql: string) {
    return { bind(...args: (string | number | null)[]) {
      return {
        async first() { return sqlite.prepare(sql).get(...args) ?? null; },
        async all() { return { success: true, results: sqlite.prepare(sql).all(...args) }; },
      };
    } };
  } } as unknown as D1Database;
  const env = { DB: db, REGISTRY_JWKS: jwks };
  return { sqlite, env, get time() { return databaseTime; }, set time(now: number) { databaseTime = now; },
    async request(path: string, jwt: string, method = 'GET', body?: unknown, headers: Record<string,string> = {}) {
      return app.request(path, { method, headers: { Authorization: `Bearer ${jwt}`, 'Content-Type': 'application/json', ...headers },
        ...(body === undefined ? {} : { body: JSON.stringify(body) }) }, env) as Promise<TestResponse>;
    },
  };
}
async function token(kind: 'punch' | 'track' | 'platform', id: string, changes: Record<string, unknown> = {}) {
  const now = Math.floor(Date.now() / 1000);
  const identity = kind === 'platform' ? {} : {
    advertise_url: kind === 'punch' ? 'quic://punch.example:4433' : 'https://track.example', identity_version: 1,
    ...(kind === 'punch' ? { public_key: 'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=' } : {}),
  };
  const scope = kind === 'punch' ? ['nodes:write:self'] : kind === 'track'
    ? ['nodes:write:self', 'discovery:punch:read'] : ['nodes:read:display'];
  return new SignJWT({ iss: 'q2q-registry', aud: 'q2q-registry', sub: id, service_type: kind,
    scope, iat: now - 1, exp: now + 3600, jti: crypto.randomUUID(), ...identity, ...changes })
    .setProtectedHeader({ alg: 'EdDSA', typ: 'JWT', kid: 'test' }).sign(pair.privateKey);
}

test('registers only signed identity, updates latest state, keeps unknown capacity and separates views', async t => {
  const f = fixture(t), punch = await token('punch', 'hk'), track = await token('track', 'track'), platform = await token('platform', 'platform');
  const registered = await f.request('/v1/nodes/self', punch, 'PUT', reportBody);
  assert.equal(registered.status, 200);
  assert.deepEqual(await registered.json(), { node_id: 'hk', last_seen: f.time, expires_at: f.time + 90, report_interval_seconds: 30 });
  assert.equal((await f.request('/v1/nodes/self', track, 'PUT', { ...reportBody, metrics: null })).status, 200);
  const discovery = await (await f.request('/v1/discovery/punches', track)).json();
  assert.equal(discovery.nodes.length, 1);
  assert.equal(discovery.nodes[0].public_key, 'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=');
  assert.equal(discovery.nodes[0].metrics.signalling_session_capacity, null);
  const display = await (await f.request('/v1/nodes', platform)).json();
  assert.equal(display.nodes.length, 2);
  assert.deepEqual(Object.keys(display.nodes[0]).sort(), ['node_id','service_type','region','online','accepting_new_connections','metrics','last_seen','expires_at'].sort());
  for (const key of ['public_key','url','advertise_url','identity_version','version','jti']) assert(!Object.hasOwn(display.nodes[0], key));
  f.time++;
  assert.equal((await f.request('/v1/nodes/self', punch, 'PUT', { ...reportBody, accepting_new_connections: false })).status, 200);
  const drained = await (await f.request('/v1/discovery/punches', track)).json();
  assert.equal(drained.nodes.length, 1);
  assert.equal(drained.nodes[0].accepting_new_connections, false);
  assert.equal(f.sqlite.prepare('SELECT COUNT(*) AS count FROM nodes').get()?.count, 2);
});

test('cross-node fields, wrong scopes, invalid metrics and unsupported bodies cannot write', async t => {
  const f = fixture(t), punch = await token('punch', 'a'), platform = await token('platform', 'platform'), track = await token('track', 'track');
  for (const body of [{ ...reportBody, node_id: 'b' }, { ...reportBody, metrics: { ...metrics, extra: 1 } },
    { ...reportBody, metrics: { ...metrics, live_bindings: -1 } }, { ...reportBody, metrics: {} }]) {
    assert.equal((await f.request('/v1/nodes/self', punch, 'PUT', body)).status, 400);
  }
  assert.equal((await f.request('/v1/nodes/self', platform, 'PUT', reportBody)).status, 403);
  assert.equal((await f.request('/v1/nodes/self', track, 'PUT', reportBody)).status, 400);
  assert.equal((await f.request('/v1/discovery/punches', punch)).status, 403);
  assert.equal((await f.request('/v1/nodes', track)).status, 403);
  assert.equal((await f.request('/v1/nodes/self', punch, 'PUT', reportBody, { 'Content-Type':'text/plain' })).status, 415);
  const tooLarge = await f.request('/v1/nodes/self', punch, 'PUT', { version: 'x'.repeat(17000) }, { 'Content-Length':'1' });
  assert.equal(tooLarge.status, 413);
  assert.equal(f.sqlite.prepare('SELECT COUNT(*) AS count FROM nodes').get()?.count, 0);
  assert.equal(f.sqlite.prepare('SELECT COUNT(*) AS count FROM report_limits').get()?.count, 0);
});

test('identity changes are atomic, reject stale tokens and preserve rows on conflict', async t => {
  const f = fixture(t), old = await token('punch', 'a');
  await f.request('/v1/nodes/self', old, 'PUT', reportBody);
  const before = f.sqlite.prepare('SELECT * FROM nodes').get();
  for (const jwt of [await token('punch', 'a', { advertise_url: 'quic://other.example:4433' }), await token('track', 'a', { identity_version: 2 })]) {
    const response = await f.request('/v1/nodes/self', jwt, 'PUT', { ...reportBody, metrics: jwt === old ? metrics : null });
    assert.equal(response.status, 409);
    assert.equal((await response.json()).code, 'identity_conflict');
    assert.deepEqual(f.sqlite.prepare('SELECT * FROM nodes').get(), before);
  }
  f.time += 18;
  const tokens = await Promise.all([2,4,3].map(version => token('punch', 'a', { identity_version: version, advertise_url: `quic://v${version}.example:4433` })));
  const responses = await Promise.all(tokens.map(jwt => f.request('/v1/nodes/self', jwt, 'PUT', reportBody)));
  assert(responses.every(r => [200,409].includes(r.status)));
  assert.equal(f.sqlite.prepare('SELECT identity_version FROM nodes').get()?.identity_version, 4);
  const latest = f.sqlite.prepare('SELECT * FROM nodes').get();
  f.time += 6;
  const stale = await f.request('/v1/nodes/self', old, 'PUT', reportBody);
  assert.equal(stale.status, 409);
  assert.equal((await stale.json()).code, 'stale_identity');
  assert.deepEqual(f.sqlite.prepare('SELECT * FROM nodes').get(), latest);
});

test('token expiry caps leases and queue-time expiry cannot create a node', async t => {
  const f = fixture(t);
  const expiry = f.time + 15;
  const jwt = await token('punch', 'short', { exp: expiry });
  const response = await f.request('/v1/nodes/self', jwt, 'PUT', reportBody);
  assert.equal(response.status, 200);
  assert.equal((await response.json()).expires_at, expiry);
  f.time = expiry;
  const expired = await f.request('/v1/nodes/self', await token('punch', 'queued', { exp: expiry }), 'PUT', reportBody);
  assert.equal(expired.status, 401);
  assert.equal((await expired.json()).code, 'token_expired');
  assert.equal(f.sqlite.prepare("SELECT node_id FROM nodes WHERE node_id='queued'").get(), undefined);
});

test('offline boundary, pagination and complete empty results are explicit', async t => {
  const f = fixture(t), reader = await token('track', 'reader'), viewer = await token('platform', 'viewer');
  for (const id of ['a','b','c']) await f.request('/v1/nodes/self', await token('punch', id), 'PUT', reportBody);
  const page = await (await f.request('/v1/discovery/punches?limit=2', reader)).json();
  assert.deepEqual(page.nodes.map((n: { punch_id:string }) => n.punch_id), ['a','b']);
  assert.equal(page.next_cursor, 'b');
  const next = await (await f.request(`/v1/discovery/punches?limit=2&after=${page.next_cursor}`, reader)).json();
  assert.deepEqual(next.nodes.map((n: { punch_id:string }) => n.punch_id), ['c']);
  assert.equal(next.next_cursor, null);
  const now = Math.floor(Date.now()/1000);
  f.sqlite.prepare('UPDATE nodes SET created_at=?, last_seen=?, expires_at=?').run(now-100,now-90,now);
  const empty = await (await f.request('/v1/discovery/punches', reader)).json();
  assert.deepEqual(empty.nodes, []);
  assert.equal(empty.next_cursor, null);
  const offline = await (await f.request('/v1/nodes', viewer)).json();
  assert.equal(offline.nodes.length,3);
  assert(offline.nodes.every((n: { online:boolean }) => !n.online));
  for (const query of ['limit=0','limit=201','after=../node','limit=1&limit=2','unknown=x']) {
    assert.equal((await f.request(`/v1/nodes?${query}`, viewer)).status,400);
  }
});

test('database failures return unavailable without leaking dependency details', async () => {
  const jwt = await token('platform', 'viewer');
  const response = await app.request('/v1/nodes', { headers:{ Authorization:`Bearer ${jwt}` } }, {
    REGISTRY_JWKS:jwks, DB:{ prepare(){ throw new Error('private dependency details'); } } as unknown as D1Database,
  });
  assert.equal(response.status,503);
  assert.deepEqual(await response.json(),{code:'registry_unavailable',detail:'Registry storage is unavailable'});
});


test('report budget allows three bursts, refills every six seconds and never renews rejected reports', async t => {
  const f = fixture(t), jwt = await token('punch', 'rate');
  for (let n=0; n<3; n++) assert.equal((await f.request('/v1/nodes/self', jwt, 'PUT', reportBody)).status, 200);
  const before = f.sqlite.prepare('SELECT * FROM nodes').get();
  const budget = f.sqlite.prepare('SELECT * FROM report_limits').get();
  for (let n=0; n<3; n++) {
    const blocked = await f.request('/v1/nodes/self', jwt, 'PUT', reportBody);
    assert.equal(blocked.status, 429);
    assert.equal(blocked.headers.get('Retry-After'), '6');
    assert.equal(blocked.headers.get('Cache-Control'), 'no-store');
    assert.equal((await blocked.json()).code, 'rate_limited');
  }
  assert.deepEqual(f.sqlite.prepare('SELECT * FROM nodes').get(), before);
  assert.deepEqual(f.sqlite.prepare('SELECT * FROM report_limits').get(), budget);
  f.time += 5;
  const nearly = await f.request('/v1/nodes/self', jwt, 'PUT', reportBody);
  assert.equal(nearly.status, 429);
  assert.equal(nearly.headers.get('Retry-After'), '1');
  f.time++;
  assert.equal((await f.request('/v1/nodes/self', jwt, 'PUT', reportBody)).status, 200);
  assert.equal((await f.request('/v1/nodes/self', jwt, 'PUT', reportBody)).status, 429);
  f.time += 60;
  for (let n=0; n<3; n++) assert.equal((await f.request('/v1/nodes/self', jwt, 'PUT', reportBody)).status, 200);
  assert.equal((await f.request('/v1/nodes/self', jwt, 'PUT', reportBody)).status, 429);
});

test('concurrent reports share one atomic budget and token rotation cannot reset it', async t => {
  const f = fixture(t), a = await token('punch', 'shared'), b = await token('punch', 'shared');
  const responses = await Promise.all(Array.from({length:12}, (_,i) => f.request('/v1/nodes/self', i%2 ? a:b, 'PUT', reportBody)));
  assert.equal(responses.filter(r => r.status===200).length, 3);
  assert.equal(responses.filter(r => r.status===429).length, 9);
  const refreshed = await token('punch', 'shared', { identity_version: 2 });
  assert.equal((await f.request('/v1/nodes/self', refreshed, 'PUT', reportBody)).status, 429);
  assert.equal((await f.request('/v1/nodes/self', await token('punch', 'independent'), 'PUT', reportBody)).status, 200);
});

test('report limiter storage errors fail closed without writing a node', async () => {
  const jwt = await token('punch', 'fail');
  const response = await app.request('/v1/nodes/self', { method:'PUT',
    headers:{Authorization:`Bearer ${jwt}`,'Content-Type':'application/json'}, body:JSON.stringify(reportBody) }, {
    REGISTRY_JWKS:jwks, DB:{ prepare(){ throw new Error('sensitive database error'); } } as unknown as D1Database,
  });
  assert.equal(response.status,503);
  assert.deepEqual(await response.json(),{code:'registry_unavailable',detail:'Registry storage is unavailable'});
});
