import assert from 'node:assert/strict';
import test from 'node:test';
import { app } from '../src/index.ts';
import { exportJWK, generateKeyPair, SignJWT } from 'jose';

test('health is public liveness and does not require D1 or signing configuration', async () => {
  const response = await app.request('/health');
  assert.equal(response.status, 200);
  assert.deepEqual(await response.json(), { service: 'q2q-registry', status: 'ok' });
  assert.equal(response.headers.get('Cache-Control'), 'no-store');
  assert.equal(response.headers.get('X-Content-Type-Options'), 'nosniff');
});

test('missing deployment keys fail closed with a safe configuration error', async () => {
  const response = await app.request('/v1/nodes', {}, { DB: {} as D1Database, REGISTRY_JWKS: '' });
  assert.equal(response.status, 503);
  assert.deepEqual(await response.json(), { code: 'registry_unavailable', detail: 'Registry authentication is unavailable' });
  assert.equal(response.headers.get('Cache-Control'), 'no-store');
});

test('versioned routes require credentials; unknown endpoints stay JSON 404', async () => {
  const pair = await generateKeyPair('EdDSA', { crv: 'Ed25519' });
  const env = { DB: {} as D1Database, REGISTRY_JWKS: JSON.stringify({ keys: [{ ...await exportJWK(pair.publicKey), kid: 'test' }] }) };
  const denied = await app.request('/v1/nodes', {}, env);
  assert.equal(denied.status, 401);
  const now = Math.floor(Date.now() / 1000);
  const token = await new SignJWT({ iss: 'q2q-registry', aud: 'q2q-registry', sub: 'platform',
    service_type: 'platform', scope: ['nodes:read:display'], iat: now, exp: now + 60, jti: 'test' })
    .setProtectedHeader({ alg: 'EdDSA', typ: 'JWT', kid: 'test' }).sign(pair.privateKey);
  const response = await app.request('/v1/missing', { headers: { Authorization: `Bearer ${token}` } }, env);
  assert.equal(response.status, 404);
  assert.deepEqual(await response.json(), { code: 'not_found', detail: 'Unknown Registry endpoint' });
  const unknown = await app.request('/missing', {}, env);
  assert.equal(unknown.status, 404);
});
