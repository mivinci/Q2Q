import assert from 'node:assert/strict';
import { test } from 'node:test';
import { runtime } from './runtime-fixture.ts';
import fixture from '../../../tests/fixtures/track/v1.json' with {type: 'json'};

test('SQLite DOs pass native state traces, quotas, sequencing and delivery fault recovery', { timeout: 30000 }, async () => {
  const mf = await runtime();
  try {
    const health = await mf.dispatchFetch('https://track.invalid/health');
    assert.equal(health.headers.get('cache-control'), 'no-store');
    assert.deepEqual(await health.json(), { service: 'q2q-track', version: '0.0.0' });
    const rejected = await mf.dispatchFetch('https://track.invalid/contract-error');
    assert.equal(rejected.status, 401);
    assert.equal(rejected.headers.get('cache-control'), 'no-store');
    assert.deepEqual(await rejected.json(), { code: 'unknown_session' });
    const unavailable = await mf.dispatchFetch('https://track.invalid/contract-unavailable');
    assert.equal(unavailable.status, 503);
    assert.deepEqual(await unavailable.json(), { code: 'no_available_punch' });
    const response = await mf.dispatchFetch('https://track.invalid/checks');
    const result = await response.json() as {passed?:string[]; error?:string};
    assert.equal(response.status, 200, JSON.stringify(result));
    assert.equal(result.passed?.length, 24, JSON.stringify(result));
  } finally { await mf.dispose(); }
});

test('multiple entries share stable DO names; namespace isolation and forced eviction retain state', { timeout: 30000 }, async () => {
  const mf = await runtime();
  try {
    const response = await mf.dispatchFetch('https://track.invalid/setup');
    assert.equal(response.status, 200);
    const saved = await response.json() as {peer_id:string; session_key:string; tombstone:{info_hash:string}};
    const second = await mf.getWorker('track-entry-two');
    const shared = await second.fetch('https://track.invalid/inspect', { method: 'POST', body: JSON.stringify(saved) });
    assert.equal((await shared.json() as {session:{session_key:string}}).session.session_key, saved.session_key);
    const isolated = await mf.getWorker('track-isolated');
    const separate = await isolated.fetch('https://track.invalid/inspect', { method: 'POST', body: JSON.stringify(saved) });
    assert.deepEqual(await separate.json(), { session: null, pending: [], alarm: null, tables: 0 });
    await mf.unsafeEvictDurableObject('track-state', 'TestPeer', { name: `peer:${saved.peer_id}` });
    await mf.unsafeEvictDurableObject('track-state', 'FaultFile', { name: `file:${saved.tombstone.info_hash}` });
    const restored = await mf.dispatchFetch('https://track.invalid/restore', { method: 'POST', body: JSON.stringify(saved) });
    assert.equal(restored.status, 200, await restored.clone().text());
    assert.deepEqual(await restored.json(), { restored: true });
  } finally { await mf.dispose(); }
});

 test('Workers HTTP matches native fixtures and exercises announce, heartbeat, query and reannounce', {timeout: 30000}, async () => {
  const mf = await runtime();
  try {
    const response = await mf.dispatchFetch('https://track.invalid/http-checks');
    const result = await response.json();
    assert.equal(response.status, 200, JSON.stringify(result));
    assert.deepEqual(result, {passed: fixture.http.length + 1});
  } finally {await mf.dispose();}
});

test('HTTP bounds and IP rate limits are shared across Worker entries', {timeout: 30000}, async () => {
  const mf = await runtime();
  try {
    const second = await mf.getWorker('track-entry-two');
    const url = 'https://track.invalid/peers/test/files';
    for (let i = 0; i < 10; i++) {
      const init = {method: 'PUT', headers: {'Content-Type': 'application/json', 'CF-Connecting-IP': '192.0.2.1'}, body: '{}'};
      const response = i % 2 ? await second.fetch(url, init) : await mf.dispatchFetch(url, init);
      assert.equal(response.status, 422);
    }
    const limited = await second.fetch(url, {method: 'PUT', headers: {'Content-Type': 'application/json', 'CF-Connecting-IP': '192.0.2.99'}, body: '{}'});
    assert.equal(limited.status, 429);
    assert.equal(limited.headers.get('retry-after'), '6');
    const wrongMedia = await mf.dispatchFetch(url, {method: 'PUT', body: '{}'});
    assert.equal(wrongMedia.status, 415);
    // Stream the final byte past the limit so the runtime consumes the complete
    // body before rejecting it. A Content-Length early rejection may reset the
    // local TCP upload before undici receives its 413 response.
    const oversizedBody = new ReadableStream<Uint8Array>({start(controller) {
      controller.enqueue(new TextEncoder().encode(' '.repeat(2 * 1024 * 1024 + 1)));
      controller.close();
    }});
    const oversized = await mf.dispatchFetch(url, {method: 'PUT', headers: {'Content-Type': 'application/json'}, body: oversizedBody, duplex: 'half'});
    assert.equal(oversized.status, 413);
  } finally {await mf.dispose();}
});

test('HTTP query preserves budget continuations and handles authoritative DO failures', {timeout: 30000}, async () => {
  const mf = await runtime('500');
  try {
    const response = await mf.dispatchFetch('https://track.invalid/query-checks');
    const result = await response.json();
    assert.equal(response.status, 200, JSON.stringify(result));
    assert.deepEqual(result, {passed: 12});
  } finally {await mf.dispose();}
});

test('hot file pages bound SQL reads and heartbeat avoids redundant cleanup alarms', {timeout: 30000}, async () => {
  const mf = await runtime();
  try {
    const response = await mf.dispatchFetch('https://track.invalid/performance-checks');
    assert.equal(response.status, 200);
    const result = await response.json() as {scan:{rows_read:number; count:number; ordered:boolean}; heartbeat_preserves_earlier_expiry_alarm:boolean; announce_schedules_expiry_after_ack:boolean};
    assert.equal(result.scan.count, 64); assert.equal(result.scan.ordered, true);
    assert.ok(result.scan.rows_read < 300, `64-candidate page read ${result.scan.rows_read} rows`);
    assert.equal(result.heartbeat_preserves_earlier_expiry_alarm, true);
    assert.equal(result.announce_schedules_expiry_after_ack, true);
  } finally {await mf.dispose();}
});
