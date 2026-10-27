import { test } from 'node:test';
import assert from 'node:assert/strict';
import { acceptsNewPeer, discoveryPageSchema } from '@q2q/punch-discovery';

const metrics = {quic_connections: 10, quic_connection_capacity: 10, websocket_connections: 10,
  websocket_connection_capacity: 10, live_bindings: 0, signalling_sessions: 0, signalling_session_capacity: null};
test('assignment capacity matches native: exclude only when both transports are known full', () => {
  assert.equal(acceptsNewPeer({accepting_new_connections: true, metrics}), false);
  assert.equal(acceptsNewPeer({accepting_new_connections: true, metrics: {...metrics, websocket_connections: 9}}), true);
  assert.equal(acceptsNewPeer({accepting_new_connections: true, metrics: {...metrics, websocket_connection_capacity: null}}), true);
  assert.equal(acceptsNewPeer({accepting_new_connections: true, metrics: null}), true);
  assert.equal(acceptsNewPeer({accepting_new_connections: false, metrics: null}), false);
});
test('RPC contract rejects excessive deadlines, invalid identity and oversized pages', () => {
  const node = {punch_id: 'punch', url: 'quic://punch.invalid:443', public_key: 'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=',
    identity_version: 1, region: null, accepting_new_connections: true, metrics: null, last_seen: 100, expires_at: 190};
  const page = {version: 1, environment: 'test', server_time: 100, expires_at: 160, nodes: [node], next_cursor: null};
  assert.ok(discoveryPageSchema.safeParse(page).success);
  assert.equal(discoveryPageSchema.safeParse({...page, expires_at: 161}).success, false);
  assert.equal(discoveryPageSchema.safeParse({...page, nodes: [{...node, url: 'quic://user:password@punch.invalid:443'}]}).success, false);
  assert.equal(discoveryPageSchema.safeParse({...page, nodes: Array(201).fill(node)}).success, false);
});
