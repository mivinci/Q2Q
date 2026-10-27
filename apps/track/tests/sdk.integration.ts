// Explicit acceptance runner: no production services, routes or credentials.
import assert from 'node:assert/strict';
import { execFile, spawn } from 'node:child_process';
import type { ChildProcess } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { test } from 'node:test';
import { runtime } from './runtime-fixture.ts';

const root = fileURLToPath(new URL('../../../', import.meta.url));
const execute = promisify(execFile);
const env: NodeJS.ProcessEnv = {...process.env, RUST_LOG: 'info'};
delete env.Q2Q_REGISTRY_TOKEN;
delete env.Q2Q_TEST_EXPIRE_URL;
async function port(): Promise<number> {
  const server = createServer();
  server.listen(0, '127.0.0.1'); await once(server, 'listening');
  const value = (server.address() as {port:number}).port;
  await new Promise<void>((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
  return value;
}
async function start(binary: string, config: string, ready: string): Promise<ChildProcess> {
  const child = spawn(join(root, 'target/debug', binary), [config], {cwd: root, env, stdio: ['ignore', 'pipe', 'pipe']});
  try {
    await new Promise<void>((resolve, reject) => {
      let output = '';
      const timeout = setTimeout(() => reject(new Error(`${binary} startup timed out: ${output}`)), 10000);
      for (const stream of [child.stdout!, child.stderr!]) stream.on('data', chunk => {output = (output + String(chunk)).slice(-8192); if (output.includes(ready)) {clearTimeout(timeout); resolve();}});
      child.once('error', error => {clearTimeout(timeout); reject(error);});
      child.once('exit', code => {clearTimeout(timeout); reject(new Error(`${binary} exited ${code}: ${output}`));});
    });
    return child;
  } catch (error) {await stop(child); throw error;}
}
async function stop(child?: ChildProcess): Promise<void> {
  if (!child || child.pid === undefined || child.exitCode !== null || child.signalCode !== null) return;
  const exited = once(child, 'exit'); child.kill('SIGTERM');
  const force = setTimeout(() => child.kill('SIGKILL'), 3000);
  try {await exited;} finally {clearTimeout(force);}
}
async function sdk(url: string, expire?: string, scenario = 'public_sdk_announces_heartbeats_paginates_and_recovers', library = false): Promise<void> {
  const result = await execute('cargo', ['test', '--offline', '-p', 'q2q-core', ...(library ? ['--lib'] : ['--test', 'track_workers']), scenario, '--', '--ignored', '--nocapture'], {
    cwd: root, env: {...env, Q2Q_TEST_TRACK_URL: url, ...(expire ? {Q2Q_TEST_EXPIRE_URL: expire} : {})}, timeout: 300000, maxBuffer: 4 * 1024 * 1024
  });
  assert.match(result.stdout, /1 passed/);
  const verified = result.stdout.split('\n').find(line => line.startsWith('P2P verified:'));
  if (verified) console.log(verified);
}

test('public SDK control-plane and verified P2P through Workers Track, Registry and native Punch', {timeout: 360000}, async () => {
  await execute('cargo', ['build', '--offline', '-p', 'q2q-punch-server', '-p', 'q2q-track-server'], {cwd: root, env, timeout: 300000, maxBuffer: 4 * 1024 * 1024});
  const temporary = await mkdtemp(join(tmpdir(), 'q2q-track-sdk-'));
  let punch: ChildProcess | undefined, track: ChildProcess | undefined;
  let worker: Awaited<ReturnType<typeof runtime>> | undefined;
  try {
    const punchPort = await port(), trackPort = await port();
    const punchConfig = join(temporary, 'punch.toml'), trackConfig = join(temporary, 'track.toml');
    await writeFile(punchConfig, `bind = "127.0.0.1:${punchPort}"\npunch_id = "sdk-punch"\n[identity]\ncert = ${JSON.stringify(join(temporary, 'cert.pem'))}\nkey = ${JSON.stringify(join(temporary, 'key.pem'))}\n`);
    const initialized = await execute(join(root, 'target/debug/q2q-punch-server'), [punchConfig, '--init-identity'], {cwd: root, env});
    const public_key = initialized.stdout.trim();
    assert.equal(Buffer.from(public_key, 'base64').length, 32);
    punch = await start('q2q-punch-server', punchConfig, 'listeners ready');
    const node = {punch_id: 'sdk-punch', url: `quic://127.0.0.1:${punchPort}`, public_key};
    await writeFile(trackConfig, `bind = "127.0.0.1:${trackPort}"\nturn_secret = "test-secret"\n[[punch]]\npunch_id = "sdk-punch"\nurl = "${node.url}"\npublic_key = "${public_key}"\n`);
    track = await start('q2q-track-server', trackConfig, 'listening');
    await sdk(`http://127.0.0.1:${trackPort}`);
    await stop(track); track = undefined;
    // Each scenario gets fresh Punch buckets and bindings; keep only its identity.
    await stop(punch); punch = await start('q2q-punch-server', punchConfig, 'listeners ready');
    // Bootstrap discovery through the actual readonly Registry RPC, without a JWT.
    worker = await runtime('5000', node);
    const url = String(await worker.ready).replace(/\/$/, '');
    await sdk(url, `${url}/sdk-expire`);
    await worker.dispose(); worker = undefined;
    await stop(punch); punch = await start('q2q-punch-server', punchConfig, 'listeners ready');
    worker = await runtime('5000', node);
    await sdk(String(await worker.ready).replace(/\/$/, ''), undefined, 'public_sdk_downloads_p2p_and_verifies_full_file_hash');
    await worker.dispose(); worker = undefined;
    await stop(punch); punch = await start('q2q-punch-server', punchConfig, 'listeners ready');
    worker = await runtime('5000', node);
    await sdk(String(await worker.ready).replace(/\/$/, ''), undefined, 'control::tests::batch_renewal_preserves_remote_partial_state_and_serializes_withdrawal', true);
  } finally {
    const cleanup = await Promise.allSettled([stop(track), stop(punch), worker?.dispose()]);
    await rm(temporary, {recursive: true, force: true});
    for (const result of cleanup) if (result.status === 'rejected') throw result.reason;
  }
});
