import assert from 'node:assert/strict';
import { test } from 'node:test';
import { runtime } from './runtime-fixture.ts';

const scenarios = {
  allocation: ['scheme-intersection','per-transport-capacity','shared-ws-capacity','unknown-capacity','paused-node','failed-allocation-no-session','eligible-node-among-full','directory-order-stable'],
  invalid: ['required-schemes','duplicate-schemes','unknown-schemes','old-transport-rejected'],
  sessions: ['cross-binding-candidate','reorder-preserves-session','query-change-invalidates','stale-candidate-hidden','rejected-heartbeat-preserves-sequence','delete-key-retirement-invalidate'],
  ledger: ['list-only-ledger-schema','atomic-ledger-and-snapshot','reorder-same-version','invalid-list-preserves-ledger'],
  'old-state': ['old-discovery-rejected','old-peer-rejected'],
};
for (const [scenario, expected] of Object.entries(scenarios)) {
  test(`endpoint HTTP and durable state contract: ${scenario}`, { timeout: 30000 }, async () => {
    const mf = await runtime();
    try {
      const response = await mf.dispatchFetch(`https://track.invalid/endpoint-checks?scenario=${scenario}`);
      const result = await response.json();
      assert.equal(response.status, 200, JSON.stringify(result));
      assert.deepEqual(result, { passed: expected });
      if (scenario === 'ledger') {
        const before = await (await mf.dispatchFetch('https://track.invalid/discovery-test?operation=identities')).json();
        const snapshot = await (await mf.dispatchFetch('https://track.invalid/discovery-test')).json();
        await mf.unsafeEvictDurableObject('track-state','TestDirectory',{name:'punch-discovery:v1'});
        assert.deepEqual(await (await mf.dispatchFetch('https://track.invalid/discovery-test?operation=identities')).json(), before);
        assert.deepEqual(await (await mf.dispatchFetch('https://track.invalid/discovery-test')).json(), snapshot);
      }
    } finally { await mf.dispose(); }
  });
}
