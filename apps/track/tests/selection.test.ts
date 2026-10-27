import { test } from 'node:test';
import assert from 'node:assert/strict';
import { peerId, validPeerId } from '../src/protocol.ts';
import { select } from '../src/selection.ts';
import type { Candidate } from '../src/state.ts';
import fixture from '../../../tests/fixtures/track/v1.json' with {type: 'json'};

const hash = 'sha256:' + '01'.repeat(32);
const ids = (await Promise.all(Array.from({length: 100}, (_, i) => peerId(new Uint8Array(32).fill(i))))).sort();
const punch = {punch_id: 'punch', url: 'quic://punch.invalid:443', public_key: 'key'};
const records: Candidate[] = ids.slice(1).map(peer_id => ({peer_id, info_hash: hash, revision: '1', complete: true, expires_at: 9999999999, public_key: 'peer-key', punch}));
const query = {hash, requester: ids[0]!, start: ids[45]!, limit: 10, deadline: Infinity, punches: new Map([['punch', punch]])};
function source(rows = records) {
  return {
    page: async (after: string | null, count: number) => rows.filter(row => after === null || row.peer_id > after).slice(0, count),
    candidate: async (reference: {peer_id: string}) => rows.find(row => row.peer_id === reference.peer_id) ?? null
  };
}
test('peer checksum rejects a changed character', () => {
  assert.ok(validPeerId(ids[0]!));
  assert.equal(validPeerId('A' + ids[0]!.slice(1)), ids[0]!.startsWith('A'));
});
test('rotating pages visit every peer once across the pivot', async () => {
  let cursor: string | undefined;
  const seen: string[] = [];
  do {
    const result = await select({...query, cursor}, source());
    seen.push(...result.peers.map(peer => peer.peer_id));
    cursor = result.next_cursor ?? undefined;
  } while (cursor);
  assert.equal(seen.length, records.length);
  assert.equal(new Set(seen).size, records.length);
  assert.equal(seen[0], ids[46]);
});
test('verification budget preserves the first unread candidate', async () => {
  let checks = 0;
  const backing = source();
  const first = await select({...query, start: ids[0]!}, {...backing, candidate: async () => { checks++; return null; }});
  assert.equal(checks, 64);
  assert.equal(first.peers.length, 0);
  assert.ok(first.next_cursor);
  const next = await select({...query, cursor: first.next_cursor!}, backing);
  assert.equal(next.peers[0]!.peer_id, ids[65]);
});
test('cursor binds the hash and requester; legacy cursor remains lexical', async () => {
  const first = await select(query, source());
  await assert.rejects(select({...query, hash: 'sha256:' + '02'.repeat(32), cursor: first.next_cursor!}, source()), /bad_request/);
  await assert.rejects(select({...query, requester: ids[2]!, cursor: first.next_cursor!}, source()), /bad_request/);
  const legacy = await select({...query, cursor: ids[90]!}, source());
  assert.equal(legacy.peers.length, 9);
  assert.equal(legacy.next_cursor, null);
});
test('stale revisions and changed Punch identities are excluded', async () => {
  const backing = source(records.slice(0, 3));
  const result = await select({...query, start: ids[0]!}, {...backing, candidate: async reference => {
    const row = await backing.candidate(reference);
    return row && {...row, revision: '2'};
  }});
  assert.deepEqual(result, {peers: [], next_cursor: null});
  const moved = await select({...query, punches: new Map([['punch', {...punch, url: 'quic://new.invalid:443'}]])}, backing);
  assert.equal(moved.peers.length, 0);
});
test('expired deadline issues no RPC; a failed RPC propagates', async () => {
  let calls = 0;
  const result = await select({...query, deadline: 0}, {...source(), page: async () => { calls++; return []; }});
  assert.equal(calls, 0);
  assert.ok(result.next_cursor);
  await assert.rejects(select(query, {...source(), candidate: async () => {throw new Error('storage_unavailable');}}), /storage_unavailable/);
});

test('native rotation and legacy fixtures have identical peers and cursor bytes', async () => {
  const vector = fixture.selection;
  const rows: Candidate[] = vector.members.filter(row => row.expires_at > vector.now).map(row => ({
    ...row, info_hash: hash, revision: '1', punch: {...punch, punch_id: row.punch_id}
  })).sort((a, b) => a.peer_id < b.peer_id ? -1 : 1);
  for (const scenario of vector.cases) {
    let cursor: string | undefined = scenario.initial_cursor ?? undefined;
    for (const expected of scenario.pages) {
      const actual = await select({...query, requester: vector.requester, start: vector.start, limit: vector.limit,
        cursor, punches: new Map([['live', punch]])}, source(rows));
      assert.deepEqual({peers: actual.peers.map(peer => ({peer_id: peer.peer_id, complete: peer.complete})), next_cursor: actual.next_cursor}, expected);
      cursor = actual.next_cursor ?? undefined;
    }
  }
});

test('RPC budget stops before another authoritative read and preserves progress', async () => {
  let calls = 0;
  const backing = source();
  const first = await select({...query, start: ids[0]!, rpcBudget: 2}, {
    page: async (after, count) => {calls++; return backing.page(after, count);},
    candidate: async reference => {calls++; return backing.candidate(reference);}
  });
  assert.equal(calls, 2);
  assert.deepEqual(first.peers.map(peer => peer.peer_id), [ids[1]]);
  const next = await select({...query, cursor: first.next_cursor!}, backing);
  assert.equal(next.peers[0]!.peer_id, ids[2]);
});
