import assert from 'node:assert/strict';
import { registerHooks } from 'node:module';
import test from 'node:test';

globalThis.registryNodesEnv = {REGISTRY_URL: 'https://registry.invalid', REGISTRY_NODES_TOKEN: 'test-only'};
registerHooks({resolve(specifier, context, next) {
  if (specifier === 'cloudflare:workers') return {url: 'data:text/javascript,export const env=globalThis.registryNodesEnv', shortCircuit: true};
  return next(specifier, context);
}});
const {getRegistryNodes} = await import('../src/lib/server/registry-nodes.ts');
const {metricsSchema} = await import('@q2q/punch-discovery');
const displayMetrics = metricsSchema.parse({quic_connections: 2, quic_connection_capacity: 10, websocket_connections: 1, websocket_connection_capacity: 10, live_bindings: 2, signalling_sessions: 1, signalling_session_capacity: null});
const nativeFetch = globalThis.fetch;
const node = (id, kind, online) => ({node_id: id, service_type: kind, region: null, online, accepting_new_connections: true,
  metrics: kind === 'punch' ? displayMetrics : null, last_seen: online ? 100 : 1, expires_at: online ? 180 : 90,
  // Deliberately present upstream fields must not become platform page data.
  public_key: 'private-to-discovery', advertise_url: 'quic://hidden.invalid:443', token: 'must-not-leak'});
test('mixed pagination validates all rows but counts/displays only Punch nodes', async t => {
  t.after(() => {globalThis.fetch = nativeFetch;});
  const paths = [];
  const pages = [
    {server_time: 120, nodes: [node('a-track', 'track', true)], next_cursor: 'a-track'},
    {server_time: 120, nodes: [node('b-punch', 'punch', true), node('c-punch', 'punch', false), node('d-track', 'track', false)], next_cursor: null}
  ];
  globalThis.fetch = async url => {paths.push(String(url)); return Response.json(pages.shift());};
  const result = await getRegistryNodes();
  assert.equal(result.state, 'ready');
  assert.equal(paths.length, 2);
  assert.equal(new URL(paths[1]).searchParams.get('after'), 'a-track');
  assert.deepEqual(result.nodes.map(n => n.node_id), ['b-punch', 'c-punch']);
  assert.equal(result.nodes.filter(n => n.online).length, 1);
  assert.deepEqual(Object.keys(result.nodes[0]).sort(), ['accepting_new_connections', 'expires_at', 'last_seen', 'metrics', 'node_id', 'online', 'region', 'service_type']);
});
test('Track-only registry is an empty ready Punch list', async t => {
  t.after(() => {globalThis.fetch = nativeFetch;});
  globalThis.fetch = async () => Response.json({server_time: 120, nodes: [node('track', 'track', true)], next_cursor: null});
  assert.deepEqual(await getRegistryNodes(), {state: 'ready', server_time: 120, nodes: []});
});
test('upstream failure and invalid hidden Track fields stay unavailable', async t => {
  t.after(() => {globalThis.fetch = nativeFetch;});
  globalThis.fetch = async () => new Response('unavailable', {status: 503});
  assert.deepEqual(await getRegistryNodes(), {state: 'unavailable', server_time: null, nodes: []});
  globalThis.fetch = async () => Response.json({server_time: 120, nodes: [{...node('track', 'track', true), metrics: {invalid: 1}}], next_cursor: null});
  assert.deepEqual(await getRegistryNodes(), {state: 'unavailable', server_time: null, nodes: []});
});
