// Local acceptance only: isolated workerd/D1, Rust services and ephemeral credentials.
// Prerequisites: npm install; cargo build -p q2q-keygen -p q2q-punch-server -p q2q-track-server.
// Run: node e2e/check-registry.mjs (about two minutes, including real lease expiry).
// Add --full after `npm run platform:build` and `cargo build -p q2q` to also
// verify two real Punch data paths and the authenticated production Platform Worker.
import assert from 'node:assert/strict';
import { createHash, createHmac, createPrivateKey, createPublicKey, randomBytes, randomUUID } from 'node:crypto';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, readFile, writeFile, rm, chmod } from 'node:fs/promises';
import { createServer } from 'node:http';
import { createServer as tcpServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const temp = await mkdtemp(join(tmpdir(), 'q2q-registry-acceptance-'));
await chmod(temp, 0o700);
const children = [];
const secrets = [];
const full = process.argv.includes('--full');
const platformOnly = process.argv.includes('--platform');
const env = { ...process.env, XDG_CONFIG_HOME: join(temp, 'config'), WRANGLER_SEND_METRICS: 'false', RUST_LOG: 'info',
  NO_PROXY: [process.env.NO_PROXY, 'localhost', '127.0.0.1', '::1'].filter(Boolean).join(',') };
let proxy;
let failure;
let checkPlatform;

function start(command, args, name, extraEnv = {}) {
  const child = spawn(command, args, { cwd: root, env: { ...env, ...extraEnv }, detached: true, stdio: ['ignore', 'pipe', 'pipe'] });
  const record = { child, name, log: '', closed: once(child, 'close') };
  child.stdout.on('data', data => { record.log += data; });
  child.stderr.on('data', data => { record.log += data; });
  children.push(record);
  return record;
}
async function stop(record) {
  if (record.child.exitCode !== null || record.child.signalCode !== null) return;
  process.kill(-record.child.pid, 'SIGTERM');
  const timer = setTimeout(() => { try { process.kill(-record.child.pid, 'SIGKILL'); } catch {} }, 4000);
  try { await record.closed; } finally { clearTimeout(timer); }
}
async function run(command, args, name) {
  const record = start(command, args, name);
  const [code] = await record.closed;
  assert.equal(code, 0, `${name} failed`);
  return record.log.trim();
}
async function port() {
  const server = tcpServer();
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const value = server.address().port;
  await new Promise(done => server.close(done));
  return value;
}
async function until(check, message, timeout = 15000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (await check()) return;
    await delay(100);
  }
  throw new Error(message);
}
async function save(name, contents) {
  const path = join(temp, name);
  await writeFile(path, contents, { mode: 0o600 });
  return path;
}

// Build valid client identities offline so both real Punch paths are selected
// without consuming Track's announce budget by trial-and-error registrations.
function peerId(publicKey) {
  const digest = createHash('sha256').update(publicKey).digest();
  const values = [];
  let bits = 0, count = 0;
  for (const byte of digest) {
    bits = (bits << 8) | byte;
    count += 8;
    while (count >= 5) { count -= 5; values.push((bits >>> count) & 31); }
  }
  if (count) values.push((bits << (5 - count)) & 31);
  let checksum = 1;
  const generators = [0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3];
  for (const value of [0, ...values, 0, 0, 0, 0, 0, 0]) {
    const top = checksum >>> 25;
    checksum = ((checksum & 0x1ffffff) << 5) ^ value;
    for (let index = 0; index < 5; index++) if ((top >>> index) & 1) checksum ^= generators[index];
  }
  checksum ^= 1;
  return values.map(value => 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567'[value]).join('')
    + Array.from({ length: 6 }, (_, index) => 'qpzry9x8gf2tvdw0s3jn54khce6mua7l'[(checksum >>> (5 * (5 - index))) & 31]).join('');
}

async function identityFor(punchId) {
  for (let attempt = 0; attempt < 128; attempt++) {
    const seed = randomBytes(32);
    const key = createPrivateKey({ key: Buffer.concat([Buffer.from('302e020100300506032b657004220420', 'hex'), seed]), format: 'der', type: 'pkcs8' });
    const id = peerId(Buffer.from(createPublicKey(key).export({ format: 'jwk' }).x, 'base64url'));
    const score = node => createHmac('sha256', 'q2q-punch-assignment-v1').update(id).update('\0').update(node).digest();
    const chosen = Buffer.compare(score('punch-acceptance'), score('punch-transfer')) > 0 ? 'punch-acceptance' : 'punch-transfer';
    if (chosen === punchId) return save(`${punchId}.identity`, Buffer.concat([Buffer.from('Q2Q-ID1\n'), seed]));
  }
  throw new Error('could not prepare identities for both Punch paths');
}
const binary = name => resolve(root, 'target/debug', name);
const wrangler = resolve(root, 'node_modules/wrangler/bin/wrangler.js');
const config = resolve(root, 'apps/registry/wrangler.jsonc');
const state = join(temp, 'state');

try {
  const privatePath = join(temp, 'signing.private.json');
  const publicPath = join(temp, 'signing.public.json');
  await run(binary('q2q-keygen'), ['keypair', '--kid', 'acceptance', '--private-key', privatePath, '--public-key', publicPath], 'key generation');
  secrets.push(JSON.parse(await readFile(privatePath, 'utf8')).d);
  const publicJwks = await readFile(publicPath, 'utf8');
  const vars = await save('registry.env', `REGISTRY_JWKS='${JSON.stringify(JSON.parse(publicJwks))}'\n`);
  await run(process.execPath, [wrangler, 'd1', 'migrations', 'apply', 'DB', '--local', '--config', config, '--persist-to', state], 'local migrations');
  const registryPort = await port();
  const registryUrl = `http://127.0.0.1:${registryPort}`;
  async function worker() {
    const record = start(process.execPath, [wrangler, 'dev', '--local', '--config', config,
      '--env-file', vars, '--persist-to', state, '--port', String(registryPort), '--inspector-port', String(await port()),
      '--ip', '127.0.0.1', '--show-interactive-dev-session=false'], 'Registry Worker');
    await until(async () => {
      try { return (await fetch(`${registryUrl}/health`)).ok; } catch { return false; }
    }, 'local Registry did not start');
    return record;
  }
  let registry = await worker();
  let outage = false;
  let displayRedirect = false;
  let redirectHits = 0;
  let displayTrace = {};
  proxy = createServer(async (request, response) => {
    if (request.url === '/__redirect_target') {
      redirectHits++;
      response.writeHead(200).end('{}');
      return;
    }
    if (displayRedirect && request.url.startsWith('/v1/nodes?')) {
      response.writeHead(302, { Location: '/__redirect_target' }).end();
      return;
    }
    // Partition the service paths; keep display readable to inspect expired
    // snapshots while Punch/Track cannot renew their leases.
    if (outage && !request.url.startsWith('/v1/nodes?')) { response.writeHead(503).end('{}'); return; }
    try {
      const chunks = [];
      for await (const chunk of request) chunks.push(chunk);
      const upstream = await fetch(`${registryUrl}${request.url}`, {
        method: request.method, headers: request.headers,
        ...(chunks.length ? { body: Buffer.concat(chunks) } : {}), signal: AbortSignal.timeout(10000),
      });
      if (request.url.startsWith('/v1/nodes?')) displayTrace = { upstreamStatus: upstream.status };
      // Node fetch decodes compressed bodies; forward headers for the decoded bytes.
      const headers = new Headers(upstream.headers);
      for (const name of ['content-encoding', 'content-length', 'transfer-encoding']) headers.delete(name);
      response.writeHead(upstream.status, Object.fromEntries(headers));
      response.end(Buffer.from(await upstream.arrayBuffer()));
    } catch (error) {
      if (request.url.startsWith('/v1/nodes?')) displayTrace = { transportError: error.name };
      response.writeHead(503).end('{}');
    }
  });
  proxy.listen(0, '127.0.0.1');
  await once(proxy, 'listening');
  const serviceRegistry = `http://127.0.0.1:${proxy.address().port}`;
  const claims = (kind, id, extra = {}) => ({ iss: 'q2q-registry', aud: 'q2q-registry', sub: id,
    service_type: kind, scope: kind === 'platform' ? ['nodes:read:display'] : kind === 'track'
      ? ['nodes:write:self', 'discovery:punch:read'] : ['nodes:write:self'], ...extra });
  async function issue(name, data) {
    const input = await save(`${name}.claims.json`, JSON.stringify(data));
    const output = join(temp, `${name}.token`);
    await run(binary('q2q-keygen'), ['token', '--key', privatePath, '--claims', input, '--output', output], 'token signing');
    const token = (await readFile(output, 'utf8')).trim();
    secrets.push(token);
    return { token, path: output };
  }
  const viewer = await issue('viewer', claims('platform', 'viewer'));
  const reader = await issue('reader', claims('track', 'reader', { advertise_url: 'https://track.example', identity_version: 1 }));
  async function api(path, token, body) {
    const response = await fetch(`${registryUrl}${path}`, { method: body ? 'PUT' : 'GET',
      headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
      ...(body ? { body: JSON.stringify(body) } : {}) });
    return { status: response.status, body: await response.json() };
  }
  async function nodes() {
    const response = await api('/v1/nodes', viewer.token);
    assert.equal(response.status, 200, 'display request failed');
    for (const node of response.body.nodes) {
      assert.deepEqual(Object.keys(node).sort(), ['node_id','service_type','region','online','accepting_new_connections','metrics','last_seen','expires_at'].sort());
    }
    return response.body.nodes;
  }
  const punchPort = await port();
  const punchBase = `bind = "127.0.0.1:${punchPort}"\npunch_id = "punch-acceptance"\n[identity]\ncert = "${join(temp, 'cert.pem')}"\nkey = "${join(temp, 'key.pem')}"\n`;
  const punchConfig = await save('punch.toml', punchBase);
  const publicKey = (await run(binary('q2q-punch-server'), [punchConfig, '--init-identity'], 'Punch identity initialization')).split('\n').at(-1);
  assert.match(publicKey, /^[A-Za-z0-9+/]{43}=$/, 'unexpected initialized public key');
  const punchIdentity = { advertise_url: `quic://127.0.0.1:${punchPort}`, public_key: publicKey, identity_version: 1 };
  const punchToken = await issue('punch', claims('punch', 'punch-acceptance', punchIdentity));
  const registryConfig = token => `\n[registry]\nurl = "${serviceRegistry}"\ntoken_file = "${token.path}"\n`;
  await writeFile(punchConfig, punchBase + registryConfig(punchToken));
  let punch = start(binary('q2q-punch-server'), [punchConfig], 'Punch');
  const trackPort = await port();
  const trackToken = await issue('track', claims('track', 'track-acceptance', { advertise_url: 'https://track.example', identity_version: 1 }));
  const trackConfig = await save('track.toml', `bind = "127.0.0.1:${trackPort}"\nturn_secret = "local-acceptance-only"\n` + registryConfig(trackToken));
  let track = start(binary('q2q-track-server'), [trackConfig], 'Track');
  await until(async () => (await nodes()).filter(node => node.online).length === 2, 'services did not register');
  await until(async () => track.log.includes('Registry Punch discovery refreshed'), 'Track discovery did not initialize');
  console.log('PASS: real Punch/Track registration, discovery and display field whitelist');

  const discovery = await api('/v1/discovery/punches', reader.token);
  assert.equal(discovery.status, 200);
  assert.equal(discovery.body.nodes[0].public_key, publicKey);
  const beforeRestart = (await nodes()).find(node => node.node_id === 'punch-acceptance').last_seen;
  await stop(punch);
  punch = start(binary('q2q-punch-server'), [punchConfig], 'restarted Punch');
  await until(async () => (await nodes()).some(node => node.node_id === 'punch-acceptance' && node.last_seen >= beforeRestart && punch.log.includes('Registry reporting active')), 'Punch did not recover with persisted identity');
  const rediscovery = await api('/v1/discovery/punches', reader.token);
  assert.equal(rediscovery.body.nodes[0].public_key, publicKey);
  const wrongToken = await issue('wrong-identity', claims('punch', 'punch-acceptance', { ...punchIdentity, public_key: Buffer.alloc(32, 9).toString('base64') }));
  const wrongConfig = await save('wrong.toml', punchBase + registryConfig(wrongToken));
  const wrong = start(binary('q2q-punch-server'), [wrongConfig], 'mismatched Punch');
  const [wrongCode] = await wrong.closed;
  assert.notEqual(wrongCode, 0, 'wrong identity started successfully');
  assert(wrong.log.includes('identity does not match'), 'identity rejection was not diagnosed');
  console.log('PASS: Punch restart retains identity; mismatched token is rejected before reporting');

  await stop(track);
  track = start(binary('q2q-track-server'), [trackConfig], 'restarted Track');
  await until(async () => track.log.includes('Registry Punch discovery refreshed') && track.log.includes('Registry Track reporting active'), 'Track did not rebuild discovery after restart');
  assert((await nodes()).some(node => node.node_id === 'track-acceptance' && node.online));
  console.log('PASS: Track cold restart rebuilds discovery and resumes its own reporting');

  if (full || platformOnly) {
    if (full) {
    const secondPort = await port();
    const secondBase = `bind = "127.0.0.1:${secondPort}"\npunch_id = "punch-transfer"\n[identity]\ncert = "${join(temp, 'second-cert.pem')}"\nkey = "${join(temp, 'second-key.pem')}"\n`;
    const secondConfig = await save('second-punch.toml', secondBase);
    const secondKey = (await run(binary('q2q-punch-server'), [secondConfig, '--init-identity'], 'second Punch identity')).split('\n').at(-1);
    assert.notEqual(secondKey, publicKey, 'Punch identities must differ');
    const secondToken = await issue('second-punch', claims('punch', 'punch-transfer', {
      advertise_url: `quic://127.0.0.1:${secondPort}`, public_key: secondKey, identity_version: 1,
    }));
    await writeFile(secondConfig, secondBase + registryConfig(secondToken));
    const second = start(binary('q2q-punch-server'), [secondConfig], 'second Punch');
    await until(async () => (await nodes()).some(node => node.node_id === 'punch-transfer' && node.online), 'second Punch did not register');
    // Restart Track so this check does not depend on its next polling deadline.
    await stop(track);
    track = start(binary('q2q-track-server'), [trackConfig], 'dual-Punch Track');
    await until(async () => track.log.includes('Registry Punch discovery refreshed'), 'dual-Punch discovery did not initialize');
    const bindings = record => (record.log.match(/binding established/g) ?? []).length;
    const selected = new Map();
    const publishers = [];
    for (let index = 0; index < 2; index++) {
      const desired = ['punch-acceptance', 'punch-transfer'][index];
      const identity = await identityFor(desired);
      const contents = randomBytes(600_000);
      const source = await save(`source-${index}.bin`, contents);
      const seed = join(temp, `source-${index}.q2q`);
      const before = [bindings(punch), bindings(second)];
      const publisher = start(binary('q2q'), ['publish', source, '--seed', seed,
        '--track', `http://127.0.0.1:${trackPort}`, '--identity', identity, '--quiet'], `publisher ${index}`, { Q2Q_HOME: join(temp, `publisher-${index}`) });
      publishers.push(publisher);
      await until(async () => bindings(punch) > before[0] || bindings(second) > before[1], 'publisher did not establish a real Punch binding', 30000);
      const assigned = bindings(punch) > before[0] ? 'punch-acceptance' : 'punch-transfer';
      assert.equal(assigned, desired, 'actual assignment disagreed with prepared identity');
      selected.set(assigned, { seed, contents });
      await until(async () => { try { return (await readFile(seed)).length > 0; } catch { return false; } }, 'publisher did not export its seed');
    }
    assert.equal(selected.size, 2, 'publishers did not exercise both Punch assignments');
    await until(async () => {
      const registered = await nodes();
      return ['punch-acceptance', 'punch-transfer'].every(id => registered.some(node => node.node_id === id && node.metrics?.live_bindings > 0));
    }, 'both Punches did not report actual live bindings', 45000);
    for (const [id, { seed, contents }] of selected) {
      // Publish and download each announce more than once during their lifecycle.
      await delay(12100);
      const output = join(temp, `${id}.download.bin`);
      const download = start(binary('q2q'), ['download', seed, '--output', output,
        '--track', `http://127.0.0.1:${trackPort}`, '--quiet'], `download from ${id}`, { Q2Q_HOME: join(temp, `download-${id}`) });
      await until(async () => download.child.exitCode !== null, `download through ${id} timed out`, 60000);
      assert.equal(download.child.exitCode, 0, `download through ${id} failed`);
      assert.equal(createHash('sha256').update(await readFile(output)).digest('hex'),
        createHash('sha256').update(contents).digest('hex'), `download through ${id} differs from source`);
    }
    for (const publisher of publishers) await stop(publisher);
    await stop(second);
    console.log('PASS: two distinct Punches have real bindings and each serves a hash-verified Peer download');
    }

    const platformPort = await port();
    const platformUrl = `http://127.0.0.1:${platformPort}`;
    const platformState = join(temp, 'platform-state');
    const platformConfig = await save('platform.json', JSON.stringify({
      name: 'q2q-platform-acceptance', compatibility_date: '2026-10-17', compatibility_flags: ['nodejs_als'],
      main: resolve(root, 'apps/platform/.svelte-kit/cloudflare/_worker.js'),
      assets: { binding: 'ASSETS', directory: resolve(root, 'apps/platform/.svelte-kit/cloudflare') },
      d1_databases: [{ binding: 'DB', database_name: 'acceptance-platform', database_id: '00000000-0000-0000-0000-000000000000', migrations_dir: resolve(root, 'apps/platform/migrations') }],
      vars: { AUTH_ORIGIN: platformUrl, REGISTRY_URL: serviceRegistry },
    }));
    const authSecret = randomBytes(32).toString('hex');
    const sessionToken = randomUUID();
    secrets.push(authSecret, sessionToken);
    const platformVars = await save('platform.env', `BETTER_AUTH_SECRET=${authSecret}\nGITHUB_CLIENT_ID=acceptance-only\nGITHUB_CLIENT_SECRET=acceptance-only\nREGISTRY_NODES_TOKEN=${viewer.token}\n`);
    await run(process.execPath, [wrangler, 'd1', 'migrations', 'apply', 'DB', '--local', '--config', platformConfig, '--persist-to', platformState], 'Platform local migrations');
    const now = Date.now();
    const sessionSql = await save('session.sql', `INSERT INTO "user" (id,name,email,emailVerified,createdAt,updatedAt) VALUES ('acceptance','Acceptance','acceptance@example.test',1,${now},${now});\nINSERT INTO session (id,expiresAt,token,createdAt,updatedAt,userId) VALUES ('acceptance',${now + 3600000},'${sessionToken}',${now},${now},'acceptance');`);
    await run(process.execPath, [wrangler, 'd1', 'execute', 'DB', '--local', '--config', platformConfig, '--persist-to', platformState, '--file', sessionSql], 'Platform local test session');
    start(process.execPath, [wrangler, 'dev', '--local', '--config', platformConfig, '--env-file', platformVars,
      '--persist-to', platformState, '--port', String(platformPort), '--inspector-port', String(await port()), '--ip', '127.0.0.1', '--show-interactive-dev-session=false'], 'Platform Worker');
    await until(async () => { try { return (await fetch(`${platformUrl}/login`)).ok; } catch { return false; } }, 'Platform Worker did not start', 30000);
    const anonymous = await fetch(`${platformUrl}/nodes`, { redirect: 'manual' });
    assert.equal(anonymous.status, 303);
    assert.equal(anonymous.headers.get('location'), '/login');
    const cookie = `better-auth.session_token=${encodeURIComponent(`${sessionToken}.${createHmac('sha256', authSecret).update(sessionToken).digest('base64')}`)}`;
    checkPlatform = async (state, offline = false) => {
      const response = await fetch(`${platformUrl}/nodes`, { headers: { cookie, 'accept-language': 'en' }, redirect: 'manual' });
      assert.equal(response.status, 200, 'authenticated node page failed');
      assert.match(response.headers.get('cache-control'), /private.*no-store/);
      const html = await response.text();
      assert(!secrets.some(secret => html.includes(secret)), 'node page exposed credentials');
      assert(!html.includes(publicKey), 'node page exposed Punch public key');
      if (state === 'ready') {
        assert(html.includes('punch-acceptance') && html.includes('track-acceptance'), `node page omitted registered services (unconfigured=${html.includes('Registry is not configured')}, unavailable=${html.includes('Node status is temporarily unavailable')}, transport=${JSON.stringify(displayTrace)})`);
        if (offline) assert(html.includes('This node is offline.'), 'node page omitted offline snapshot explanation');
      } else {
        assert(html.includes('Node status is temporarily unavailable'), 'node page did not distinguish Registry failure');
      }
    };
    await checkPlatform('ready');
    displayRedirect = true;
    await checkPlatform('unavailable');
    assert.equal(redirectHits, 0, 'Platform followed Registry redirect with a machine credential');
    displayRedirect = false;
    console.log('PASS: authenticated production Platform Worker renders node status; anonymous access redirects and credentials stay server-side');
  }

  const body = { version: 'acceptance', region: null, accepting_new_connections: true, metrics: (await nodes()).find(node => node.node_id === 'punch-acceptance').metrics };
  const versions = await Promise.all([2,4,3].map(version => issue(`concurrent-${version}`,
    claims('punch', 'concurrent', { advertise_url: `quic://v${version}.example:4433`, public_key: publicKey, identity_version: version }))));
  const concurrent = await Promise.all(versions.map(token => api('/v1/nodes/self', token.token, body)));
  assert(concurrent.every(result => [200,409].includes(result.status)), 'unexpected concurrent response');
  const latest = (await api('/v1/discovery/punches', reader.token)).body.nodes.find(node => node.punch_id === 'concurrent');
  assert.equal(latest.identity_version, 4);
  assert.equal(latest.url, 'quic://v4.example:4433');
  // All three attempts consume the shared budget, including conflicts.
  const stale = await api('/v1/nodes/self', versions[0].token, body);
  assert.equal(stale.status, 429, 'concurrent requests did not share node budget');
  await delay(6100);
  const staleAfterRefill = await api('/v1/nodes/self', versions[0].token, body);
  assert.equal(staleAfterRefill.status, 409);
  assert.equal(staleAfterRefill.body.code, 'stale_identity');
  console.log('PASS: real D1 concurrent identity updates cannot roll back; shared limiter applies');

  outage = true;
  const leases = await nodes();
  const untilExpiry = Math.max(...leases.map(node => node.expires_at)) * 1000 + 1000;
  await stop(registry);
  if (checkPlatform) await checkPlatform('unavailable');
  registry = await worker();
  console.log('PASS: Registry restarted with isolated persistent D1; simulating outage through original leases');
  while (Date.now() < untilExpiry) await delay(Math.min(1000, untilExpiry - Date.now()));
  assert((await nodes()).every(node => !node.online), 'failed reports extended node leases');
  assert(punch.child.exitCode === null && track.child.exitCode === null, 'Registry outage stopped a service');
  const empty = await api('/v1/discovery/punches', reader.token);
  assert.equal(empty.status, 200);
  assert.deepEqual(empty.body.nodes, []);
  if (checkPlatform) await checkPlatform('ready', true);
  console.log('PASS: real lease expiry preserves offline display, removes discovery and leaves services running');
  outage = false;
  await until(async () => (await nodes()).filter(node => node.online).length === 2, 'services did not automatically recover after Registry restoration', 45000);
  await until(async () => (track.log.match(/Registry Punch discovery refreshed/g) ?? []).length >= 2, 'Track did not recover discovery', 45000);
  console.log('PASS: Punch/Track reporting and discovery recover without service restart');
  if (checkPlatform) {
    await checkPlatform('ready');
    console.log('PASS: Platform distinguishes Registry failure, offline history and recovered node status');
  }
} catch (error) {
  failure = error;
} finally {
  for (const record of [...children].reverse()) await stop(record);
  if (proxy) { proxy.closeAllConnections(); await new Promise(done => proxy.close(done)); }
  const logs = children.map(record => record.log).join('\n');
  if (secrets.some(secret => logs.includes(secret))) failure = new Error('a credential appeared in captured process logs');
  if (failure && process.env.Q2Q_E2E_DIAGNOSTICS === '1') {
    for (const record of children.filter(record => /publisher|download|Track|Punch|Platform/.test(record.name))) {
      let safe = record.log.split('\n').slice(-12).join('\n');
      for (const secret of secrets) safe = safe.replaceAll(secret, '[redacted]');
      console.error(`${record.name}:\n${safe}`);
    }
  }
  await rm(temp, { recursive: true, force: true });
}
if (failure) {
  // Commands never receive credential values as arguments; avoid printing raw
  // response bodies or child output if an acceptance assertion fails.
  console.error(`FAIL: ${failure.message}`);
  process.exitCode = 1;
} else {
  console.log('PASS: captured logs contain no issued JWT/private signing material; processes and temporary state removed');
}
