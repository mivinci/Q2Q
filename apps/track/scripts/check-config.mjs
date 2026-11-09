import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const track = JSON.parse(readFileSync(new URL('../wrangler.jsonc', import.meta.url), 'utf8'));
const registry = JSON.parse(readFileSync(new URL('../../registry/wrangler.jsonc', import.meta.url), 'utf8'));
const binding = track.services?.filter(service => service.binding === 'REGISTRY');
assert.equal(binding?.length, 1, 'Track requires one Registry binding');
assert.equal(binding[0].entrypoint, 'PunchDiscovery', 'Bind only the readonly named entrypoint');
assert.equal(binding[0].service, registry.name, 'Registry service must match this environment');
assert.ok(track.vars.REGISTRY_ENVIRONMENT, 'Track must declare its expected Registry environment');
assert.equal(track.vars.REGISTRY_ENVIRONMENT, registry.vars.DISCOVERY_ENVIRONMENT, 'Track/Registry environments must match');
assert.equal(registry.main, 'src/worker.ts', 'Registry must export the named RPC entrypoint');
console.log('Track/Registry readonly binding and environment configuration match');

const expectedClasses = {PEERS:'PeerStateEndpoints',FILES:'FileIndexEndpoints',DISCOVERY:'DiscoveryStateEndpoints',RATE_LIMIT:'RateLimiterEndpoints'};
for (const [name,className] of Object.entries(expectedClasses)) {
  assert.equal(track.durable_objects.bindings.find(binding=>binding.name===name)?.class_name,className,'All state bindings must use the endpoint-list generation');
}
assert.deepEqual(track.migrations.find(migration=>migration.tag==='v3-endpoints')?.new_sqlite_classes,Object.values(expectedClasses));
console.log('Track endpoint-list state generation and migration are coordinated');
