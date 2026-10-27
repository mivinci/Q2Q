import { test } from 'node:test';
import assert from 'node:assert/strict';
import { networkPrefix, rateObjectName } from '../src/rate-key.ts';

test('native network prefixes share IPv4 /24 and canonical IPv6 /64 quotas', () => {
  assert.equal(networkPrefix('192.0.2.1'), '192.0.2.0/24');
  assert.equal(networkPrefix('192.0.2.254'), networkPrefix('192.0.2.1'));
  assert.notEqual(networkPrefix('192.0.3.1'), networkPrefix('192.0.2.1'));
  assert.equal(networkPrefix('2001:0db8:0001:0002::1'), '2001:db8:1:2::/64');
  assert.equal(networkPrefix('2001:db8:1:2:3:4:5:6'), networkPrefix('2001:db8:1:2::1'));
  assert.equal(networkPrefix('::ffff:192.0.2.1'), '0:0:0:0::/64');
  assert.equal(networkPrefix('invalid'), 'local');
});

test('gateway and load observer share canonical opaque network object names', async () => {
  const first = await rateObjectName('192.0.2.1');
  assert.match(first, /^network:[0-9a-f]{64}$/);
  assert.equal(await rateObjectName('192.0.2.99'), first);
  assert.notEqual(await rateObjectName('192.0.3.1'), first);
});
