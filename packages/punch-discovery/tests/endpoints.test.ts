import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createPublicKey, verify } from 'node:crypto';
import vectors from '../../../tests/fixtures/punch/endpoints.json' with { type: 'json' };
import credential from '../../../tests/fixtures/punch/credential.json' with { type: 'json' };
import { canonicalPunchEndpoint, punchEndpointSchema, punchEndpointsSchema, punchSchemesSchema,
  punchAssignmentSchema, punchIdentitySchema } from '../index.ts';

for (const vector of vectors.endpoints) {
  test(`shared endpoint: ${vector.name}`, () => {
    const result = punchEndpointSchema.safeParse(vector.input);
    assert.equal(result.success, vector.canonical !== null);
    if (result.success) {
      assert.equal(result.data, vector.canonical);
      assert.equal(canonicalPunchEndpoint(result.data), result.data, 'canonicalization must be idempotent');
    }
  });
}
for (const [name, cases, schema] of [
  ['set', vectors.sets, punchEndpointsSchema],
  ['capabilities', vectors.schemes, punchSchemesSchema],
] as const) {
  for (const vector of cases) {
    test(`shared ${name}: ${vector.name}`, () => {
      const before = JSON.stringify(vector.input);
      const result = schema.safeParse(vector.input);
      assert.equal(result.success, vector.canonical !== null);
      if (result.success) assert.deepEqual(result.data, vector.canonical);
      assert.equal(JSON.stringify(vector.input), before, 'parsing must not sort caller-owned input');
    });
  }
}

test('URL bounds apply before and after canonicalization', () => {
  const prefix = 'wss://punch.invalid/';
  assert.ok(punchEndpointSchema.safeParse(prefix + 'x'.repeat(2048 - prefix.length)).success);
  assert.equal(punchEndpointSchema.safeParse(prefix + 'x'.repeat(2049 - prefix.length)).success, false);
  // URL encoding expands non-ASCII paths: stored/signed URLs have the same budget.
  assert.equal(punchEndpointSchema.safeParse(prefix + '中'.repeat(300)).success, false);
  for (const input of [null, 1, true, {}, []]) assert.equal(punchEndpointSchema.safeParse(input).success, false);
});

test('endpoint errors do not echo query credentials', () => {
  const result = punchEndpointSchema.safeParse('wss://user@punch.invalid/ws?secret=do-not-echo');
  assert.equal(result.success, false);
  if (!result.success) assert.equal(result.error.message.includes('do-not-echo'), false);
});

test('endpoint count and invalid elements are never silently truncated', () => {
  const values = Array.from({ length: 8 }, (_, i) => `wss://punch.invalid/${i}`);
  assert.equal(punchEndpointsSchema.parse(values).length, 8);
  assert.equal(punchEndpointsSchema.safeParse([...values, 'wss://punch.invalid/8']).success, false);
});

const claims = credential.claims;
const identity = { endpoints: claims.endpoints, public_key: claims.public_key, identity_version: claims.identity_version };
const assignment = { punch_id: claims.sub, endpoints: claims.endpoints, public_key: claims.public_key };

test('list-only assignment and signed identity use exactly the same canonical endpoints', () => {
  assert.deepEqual(punchIdentitySchema.parse(identity), identity);
  assert.deepEqual(punchAssignmentSchema.parse(assignment), assignment);
  assert.deepEqual(punchIdentitySchema.parse({ ...identity, endpoints: [...identity.endpoints].reverse() }), identity);
  for (const field of ['url', 'advertise_url', 'websocket_url']) {
    assert.equal(punchAssignmentSchema.safeParse({ ...assignment, [field]: 'quic://old.invalid:4433' }).success, false);
    assert.equal(punchIdentitySchema.safeParse({ ...identity, [field]: 'quic://old.invalid:4433' }).success, false);
  }
  assert.equal(punchAssignmentSchema.safeParse({ punch_id: claims.sub, url: 'quic://old.invalid:4433', public_key: claims.public_key }).success, false);
});

test('assignment ID, key and identity version preserve existing safe wire bounds', () => {
  for (const punch_id of ['', '../node', 'x'.repeat(129)]) assert.equal(punchAssignmentSchema.safeParse({ ...assignment, punch_id }).success, false);
  for (const public_key of ['', 'invalid', claims.public_key.slice(0, -1), 'A'.repeat(42) + 'B=']) {
    assert.equal(punchAssignmentSchema.safeParse({ ...assignment, public_key }).success, false);
  }
  for (const identity_version of [0, -1, 1.5, Number.MAX_SAFE_INTEGER + 1, '1']) {
    assert.equal(punchIdentitySchema.safeParse({ ...identity, identity_version }).success, false);
  }
  assert.ok(punchIdentitySchema.safeParse({ ...identity, identity_version: Number.MAX_SAFE_INTEGER }).success);
});

test('public deterministic JWT covers endpoints, key and identity version verbatim', () => {
  const parts = credential.token.split('.');
  const input = `${parts[0]}.${parts[1]}`;
  const key = createPublicKey({ key: credential.public_jwk, format: 'jwk' });
  const signature = Buffer.from(parts[2]!, 'base64url');
  assert.deepEqual(JSON.parse(Buffer.from(parts[0]!, 'base64url').toString()), credential.header);
  assert.deepEqual(JSON.parse(Buffer.from(parts[1]!, 'base64url').toString()), claims);
  assert.ok(verify(null, Buffer.from(input), key, signature));
  for (const changes of [
    { endpoints: [...claims.endpoints].reverse() },
    { endpoints: ['quic://punch.invalid:4433'] },
    { endpoints: ['quic://punch.invalid:4433', 'wss://proxy.invalid:8443/changed'] },
    { public_key: 'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=' },
    { identity_version: claims.identity_version + 1 },
  ]) {
    const body = Buffer.from(JSON.stringify({ ...claims, ...changes })).toString('base64url');
    assert.equal(verify(null, Buffer.from(`${parts[0]}.${body}`), key, signature), false);
  }
  // Normalization is for identity comparison only; never normalize the JWT body
  // before checking its signature. Reordering still invalidates those signed bytes.
});
