import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFile, readdir, writeFile } from 'node:fs/promises';
import { arch, cpus, platform } from 'node:os';
import { fileURLToPath } from 'node:url';
import { build } from 'esbuild';
import { Miniflare, Log, LogLevel } from 'miniflare';
import type { MiniflareWorkerConfig } from 'miniflare';
import { base64, canonicalTicket, decodeBase64, peerId, sessionMac, stringifyWire } from '../src/protocol.ts';
import { rateObjectName } from '../src/rate-key.ts';

const root = new URL('../../../', import.meta.url);
const arg = (name: string, fallback: string) => process.argv.find(value => value.startsWith(`--${name}=`))?.slice(name.length + 3) ?? fallback;
const seconds = Number(arg('seconds', '20')), rate = Number(arg('rate', '50'));
const maximum = Number(arg('peers', '1000'));
assert.ok(Number.isInteger(maximum) && maximum >= 2 && maximum <= 10000);
assert.ok(Number.isFinite(seconds) && seconds >= 1 && seconds <= 300);
assert.ok(Number.isInteger(rate) && rate >= 1 && rate <= 2000);
type Profile = {peers:number; holdings:number; layout:'distributed'|'hot'; entries:number; renewal_mode?:'single'|'batch'};
type Metrics = {incarnation:string; alarm_sets:number; kv_gets:number; kv_puts:number; kv_deletes:number; statements:number; rows_read:number; rows_written:number; rpc:number; alarms:number; pending:number; database_bytes:number};
type Target = {kind:'PEERS'|'FILES'|'DISCOVERY'|'RATE_LIMIT'; name:string};
type Participant = {id:string; key:CryptoKey; pub:string; session:string; seq:bigint; hashes:string[]; ip:string; tail:Promise<unknown>; renewal:number};

async function map<T, R>(values: T[], width: number, fn: (value:T, index:number) => Promise<R>): Promise<R[]> {
  const results = new Array<R>(values.length);
  let next = 0;
  await Promise.all(Array.from({length: Math.min(width, values.length)}, async () => {
    while (next < values.length) {const index = next++; results[index] = await fn(values[index]!, index);}
  }));
  return results;
}
async function setup() {
  const bundles = await Promise.all(['./benchmark-worker.ts', '../../registry/src/worker.ts'].map(name => build({
    entryPoints: [fileURLToPath(new URL(name, import.meta.url))], bundle: true, write: false,
    format: 'esm', platform: 'browser', target: 'es2022', external: ['cloudflare:workers']
  })));
  const manifest = (index:number) => ({mainModule: 'index.js', modules: {'index.js': {type: 'esm' as const, contents: bundles[index]!.outputFiles[0]!.text}}});
  function entry(name: string): MiniflareWorkerConfig {
    return {name, compatibilityDate: '2026-10-26', manifest: manifest(0), exports: name === 'benchmark-state' ? {
      MeasuredPeer: {type: 'durable-object', storage: 'sqlite', unsafePreventEviction: true}, MeasuredFile: {type: 'durable-object', storage: 'sqlite', unsafePreventEviction: true},
      MeasuredDiscovery: {type: 'durable-object', storage: 'sqlite', unsafePreventEviction: true}, MeasuredLimiter: {type: 'durable-object', storage: 'sqlite', unsafePreventEviction: true}
    } : {}, env: {
      PEERS: {type: 'durable-object', worker: 'benchmark-state', exportName: 'MeasuredPeer'},
      FILES: {type: 'durable-object', worker: 'benchmark-state', exportName: 'MeasuredFile'},
      DISCOVERY: {type: 'durable-object', worker: 'benchmark-state', exportName: 'MeasuredDiscovery'},
      RATE_LIMIT: {type: 'durable-object', worker: 'benchmark-state', exportName: 'MeasuredLimiter'},
      REGISTRY: {type: 'worker', worker: 'benchmark-registry', exportName: 'PunchDiscovery'},
      REGISTRY_ENVIRONMENT: {type: 'text', value: 'test'}, TURN_URL: {type: 'text', value: 'turn:127.0.0.1:9'},
      TURN_SECRET: {type: 'text', value: 'test-only'}
    }};
  }
  const mf = new Miniflare({workers: [{config: entry('benchmark-state')}, {config: entry('benchmark-two')}, {config: {
    name: 'benchmark-registry', compatibilityDate: '2026-10-26', manifest: manifest(1), exports: {PunchDiscovery: {type: 'worker'}},
    env: {DB: {type: 'd1', id: 'benchmark-registry'}, DISCOVERY_ENVIRONMENT: {type: 'text', value: 'test'}}
  }}], host: '127.0.0.1', port: 0, log: new Log(LogLevel.ERROR), telemetry: {enabled: false}});
  let renewal: ReturnType<typeof setInterval> | undefined;
  try {
    const db = await mf.getD1Database('DB', 'benchmark-registry');
    for (const migration of ['0001_nodes.sql', '0002_report_limits.sql', '0003_punch_management.sql']) {
      const sql = (await readFile(new URL(`apps/registry/migrations/${migration}`, root), 'utf8')).replace(/^\s*--.*$/gm, '');
      for (const statement of sql.split(';').map(value => value.trim()).filter(Boolean)) await db.prepare(statement).run();
    }
    const now = Math.floor(Date.now() / 1000);
    await db.prepare("INSERT INTO nodes VALUES('benchmark-punch', 'punch', 1, 'quic://punch.invalid:443', ?, 'test', NULL, 1, NULL, ?, ?, ?)")
      .bind(base64(new Uint8Array(32)), now, now, now + 90).run();
    // Long profiles refresh real Registry leases; this is the equivalent of native reports.
    renewal = setInterval(() => {
      const time = Math.floor(Date.now() / 1000);
      void db.prepare('UPDATE nodes SET last_seen=?, expires_at=?').bind(time, time + 90).run().catch(() => {});
    }, 30000);
    const entries = await Promise.all(['benchmark-state', 'benchmark-two'].map(name => mf.getWorker(name)));
    return {mf, entries, async close() {clearInterval(renewal); await mf.dispose();}};
  } catch (error) {clearInterval(renewal); await mf.dispose(); throw error;}
}
const latency = (values:number[]) => {
  const sorted = [...values].sort((a, b) => a - b);
  const percentile = (fraction:number) => sorted.length ? Math.round(sorted[Math.min(sorted.length - 1, Math.ceil(sorted.length * fraction) - 1)]! * 100) / 100 : null;
  return {count: sorted.length, p50_ms: percentile(.5), p95_ms: percentile(.95), p99_ms: percentile(.99)};
};
async function run(profile: Profile) {
  console.log(`Preparing ${JSON.stringify(profile)}`);
  const runtime = await setup(), targets = new Map<string, Target>();
  const target = (kind: Target['kind'], name:string) => targets.set(`${kind}:${name}`, {kind, name});
  target('DISCOVERY', 'punch-discovery:v1');
  try {
    const clients = await map(Array.from({length: profile.peers}, (_, index) => index), 16, async index => {
      const pair = await crypto.subtle.generateKey('Ed25519', true, ['sign', 'verify']) as CryptoKeyPair;
      const publicKey = new Uint8Array(await crypto.subtle.exportKey('raw', pair.publicKey));
      const id = await peerId(publicKey), ip = `10.${(index >> 8) & 255}.${index & 255}.1`;
      const hashes = Array.from({length: profile.holdings}, (_, file) => 'sha256:' + createHash('sha256').update(`${profile.layout === 'hot' ? 'shared' : id}:${file}`).digest('hex'));
      target('PEERS', `peer:${id}`); target('RATE_LIMIT', await rateObjectName(ip));
      for (const hash of hashes) target('FILES', `file:${hash}`);
      return {id, key: pair.privateKey, pub: base64(publicKey), session: '', seq: 0n, hashes, ip, tail: Promise.resolve(), renewal: 0} as Participant;
    });
    async function announce(client:Participant, index:number, hashes = client.hashes) {
      const now = BigInt(Math.floor(Date.now() / 1000));
      const ticket = {v: 2n, iss: client.id, pub: client.pub, iat: now, exp: now + 60n, aud: 'track', sig: ''};
      ticket.sig = base64(new Uint8Array(await crypto.subtle.sign('Ed25519', client.key, Uint8Array.from(canonicalTicket(ticket)).buffer)));
      const body = stringifyWire({token: ticket, ttl: profile.renewal_mode ? 300n : 3600n, add: hashes, complete: hashes});
      const started = performance.now();
      const response = await runtime.entries[index % profile.entries]!.fetch(`https://track.invalid/peers/${client.id}/files`, {
        method: 'PUT', headers: {'Content-Type': 'application/json', 'CF-Connecting-IP': client.ip}, body
      });
      const result = await response.json() as {session_key?:string; code?:string};
      if (response.ok) {client.session = result.session_key!; client.seq = 0n;}
      return {ok: response.ok, status: response.status, ms: performance.now() - started, code: result.code};
    }
    const seeded = await map(clients, 8, announce);
    assert.ok(seeded.every(result => result.ok), JSON.stringify(seeded.filter(result => !result.ok).slice(0, 3)));
    const observe = async () => map([...targets.values()], 16, async item => {
      const response = await runtime.entries[0]!.fetch('https://track.invalid/measure', {method: 'POST', body: JSON.stringify({...item, reset: false})});
      if (!response.ok) throw new Error(await response.text());
      return {target: item, data: await response.json() as Metrics};
    });
    await new Promise(resolve => setTimeout(resolve, 2100)); // Drain initial announce alarms before warm-load sampling.
    const before = await observe();
    const samples: {kind:string; ok:boolean; status:number; ms:number; scheduled_ms:number; code?:string}[] = [];
    const count = Math.max(1, Math.floor(rate * seconds)), started = performance.now();
    const harnessCpu = process.cpuUsage();
    let inFlight = 0, maximumInFlight = 0;
    const waiters: (() => void)[] = [];
    const acquire = async () => {if (inFlight >= 32) {await new Promise<void>(resolve => waiters.push(resolve)); return;} inFlight++; maximumInFlight = Math.max(maximumInFlight, inFlight);};
    const release = () => {const next = waiters.shift(); if (next) next(); else inFlight--;};
    for (let operation = 0; operation < count; operation++) {
      const scheduled = started + operation * 1000 / rate;
      const wait = scheduled - performance.now();
      if (wait > 0) await new Promise(resolve => setTimeout(resolve, wait));
      const clientIndex = (operation * 37 + Math.floor(operation / clients.length)) % clients.length, client = clients[clientIndex]!;
      const kind = profile.renewal_mode === 'batch' ? (operation % 100 >= 87 ? 'announce' : operation % 100 < 25 ? 'query' : 'heartbeat') : profile.renewal_mode ? (operation % 100 >= 18 ? 'announce' : operation % 100 >= 13 ? 'query' : 'heartbeat') : (operation % 20 === 19 ? 'announce' : operation % 20 >= 14 ? 'query' : 'heartbeat');
      client.tail = client.tail.then(async () => {
        await acquire();
        try {
          if (kind === 'announce') {
            const hashes = profile.renewal_mode === 'single' ? [client.hashes[client.renewal++ % client.hashes.length]!] : client.hashes;
            const result = await announce(client, operation, hashes);
            samples.push({kind, ...result, scheduled_ms: performance.now() - scheduled});
            return;
          }
          client.seq++;
          const now = BigInt(Math.floor(Date.now() / 1000));
          const auth = {seq: client.seq, timestamp: now, mac: await sessionMac(decodeBase64(client.session, 32), client.id, client.seq, now)};
          const isQuery = kind === 'query';
          const parameters = new URLSearchParams({peer_id: client.id, seq: String(auth.seq), timestamp: String(now), mac: auth.mac, limit: '10'});
          const hash = clients[(clientIndex + 1) % clients.length]!.hashes[0]!;
          const path = isQuery ? `/files/${hash}/peers?${parameters}` : `/peers/${client.id}/heartbeat`;
          const body = isQuery ? undefined : stringifyWire(auth);
          const beginning = performance.now();
          const response = await runtime.entries[operation % profile.entries]!.fetch('https://track.invalid' + path, {method: isQuery ? 'GET' : 'POST',
            headers: {'Content-Type': 'application/json', 'CF-Connecting-IP': client.ip}, ...(body ? {body} : {})});
          const result = await response.json() as {code?:string};
          samples.push({kind, ok: response.ok, status: response.status, code: result.code, ms: performance.now() - beginning, scheduled_ms: performance.now() - scheduled});
        } catch (error) {samples.push({kind, ok: false, status: 0, code: String(error), ms: performance.now() - scheduled, scheduled_ms: performance.now() - scheduled});}
        finally {release();}
      });
    }
    await Promise.all(clients.map(client => client.tail));
    const remainder = started + seconds * 1000 - performance.now();
    if (remainder > 0) await new Promise(resolve => setTimeout(resolve, remainder));
    const elapsed = performance.now() - started, cpu = process.cpuUsage(harnessCpu);
    const after = await observe();
    const storage = Object.fromEntries(['PEERS', 'FILES', 'DISCOVERY', 'RATE_LIMIT'].map(kind => {
      const summary = {objects: 0, alarm_sets: 0, kv_gets: 0, kv_puts: 0, kv_deletes: 0, statements: 0, rows_read: 0, rows_written: 0, rpc: 0, alarms: 0, pending: 0, database_bytes: 0};
      for (let index = 0; index < after.length; index++) {
        const current = after[index]!, previous = before[index]!;
        if (current.target.kind !== kind) continue;
        summary.objects++;
        for (const key of ['alarm_sets', 'kv_gets', 'kv_puts', 'kv_deletes', 'statements', 'rows_read', 'rows_written', 'rpc', 'alarms'] as const) summary[key] += current.data[key] - previous.data[key];
        summary.pending += current.data.pending; summary.database_bytes += current.data.database_bytes;
      }
      return [kind, summary];
    }));
    const samplingValid = after.every((item, index) => item.data.incarnation === before[index]!.data.incarnation);
    const errors = samples.filter(sample => !sample.ok);
    const p99 = latency(samples.map(sample => sample.scheduled_ms)).p99_ms;
    const result = {profile, seed: latency(seeded.map(item => item.ms)), target_rps: rate, seconds, actual_elapsed_ms: Math.round(elapsed),
      completed_rps: Math.round(samples.length * 100000 / elapsed) / 100, maximum_in_flight: maximumInFlight,
      request_latency: latency(samples.map(sample => sample.ms)), scheduled_latency: latency(samples.map(sample => sample.scheduled_ms)),
      methods: Object.fromEntries(['announce', 'heartbeat', 'query'].map(kind => [kind, latency(samples.filter(sample => sample.kind === kind).map(sample => sample.ms))])),
      sampling_valid: samplingValid, errors: errors.length, errors_by_status: Object.fromEntries([...new Set(errors.map(sample => sample.status))].map(status => [String(status), errors.filter(sample => sample.status === status).length])), error_examples: errors.slice(0, 3), storage, worker_cpu_ms: null,
      harness_cpu_ms: (cpu.user + cpu.system) / 1000,
      meets_local_gate: samplingValid && errors.length === 0 && (p99 ?? Infinity) < 1000 && latency(samples.map(sample => sample.scheduled_ms)).p95_ms! < 250 && storage.PEERS!.pending === 0};
    console.log(JSON.stringify(result));
    return result;
  } finally {await runtime.close();}
}

const selectedProfiles = arg('profiles', 'all');
if (!['all', 'sdk', 'sdk-batched', 'baseline', 'smoke'].includes(selectedProfiles)) throw new Error('Unknown --profiles value');
const profiles: Profile[] = selectedProfiles === 'sdk-batched' ? [{peers: maximum, holdings: 32, layout: 'hot', entries: 2, renewal_mode: 'batch'}] : selectedProfiles === 'sdk' ? [{peers: maximum, holdings: 32, layout: 'hot', entries: 2, renewal_mode: 'single'}] : selectedProfiles === 'baseline' ? [{peers: maximum, holdings: 1, layout: 'hot', entries: 2}, {peers: maximum, holdings: 32, layout: 'hot', entries: 2}] : selectedProfiles === 'smoke' ? [{peers: 4, holdings: 1, layout: 'hot', entries: 2}] : [
  {peers: Math.min(100, maximum), holdings: 1, layout: 'distributed', entries: 1},
  {peers: maximum, holdings: 1, layout: 'distributed', entries: 1},
  {peers: maximum, holdings: 1, layout: 'hot', entries: 1},
  {peers: maximum, holdings: 1, layout: 'hot', entries: 2},
  {peers: maximum, holdings: 32, layout: 'hot', entries: 2}
];
const results = [];
for (const profile of profiles) results.push(await run(profile));
const sources = createHash('sha256');
for (const directory of ['apps/track/src', 'apps/registry/src', 'packages/punch-discovery']) {
  for (const file of (await readdir(new URL(directory + '/', root))).filter(name => name.endsWith('.ts')).sort()) {
    sources.update(`${directory}/${file}\0`); sources.update(await readFile(new URL(`${directory}/${file}`, root)));
  }
}
const report = {date: new Date().toISOString(), environment: {platform: platform(), arch: arch(), node: process.version, logical_cpus: cpus().length},
  source_sha256: sources.digest('hex'), scope: 'Local workerd only; DO eviction disabled and incarnations checked for measurement stability. Alarm-set and KV API calls counted separately from SQL cursors. Observer calls/SQL excluded; background alarms during sampling included. Harness CPU excludes workerd. No cloud CPU, GB-s or geographic scaling claim.', results};
const output = arg('output', '/tmp/q2q-track-benchmark.json');
await writeFile(output, JSON.stringify(report, null, 2) + '\n');
console.log(`Report: ${output}`);
if (results.some(result => !result.meets_local_gate)) process.exitCode = 1;
