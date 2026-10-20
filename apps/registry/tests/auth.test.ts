import assert from 'node:assert/strict';
import test from 'node:test';
import { Hono } from 'hono';
import { exportJWK, generateKeyPair, SignJWT } from 'jose';
import { AuthError, ForbiddenError, RegistryConfigError, authentication, authorize, createVerifier, permissionRequired } from '../src/auth.ts';
import type { RegistryEnv } from '../src/auth.ts';

const now = 1_800_000_000;
const pair = await generateKeyPair('EdDSA', { crv: 'Ed25519' });
const other = await generateKeyPair('EdDSA', { crv: 'Ed25519' });
const jwk = { ...await exportJWK(pair.publicKey), kid: 'key-1', alg: 'EdDSA' };
const jwks = JSON.stringify({ keys: [jwk] });
const verify = createVerifier(jwks);
const base = { iss: 'q2q-registry', aud: 'q2q-registry', sub: 'punch-1',
  service_type: 'punch', scope: ['nodes:write:self'], iat: now, exp: now + 3600, jti: 'random-id',
  advertise_url: 'quic://punch.example:4433', public_key: 'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=', identity_version: 1 };
async function token(changes = {}, key = pair.privateKey, kid = 'key-1') {
  return new SignJWT({ ...base, ...changes }).setProtectedHeader({ alg: 'EdDSA', typ: 'JWT', kid }).sign(key);
}
async function check(changes = {}) { return verify(`Bearer ${await token(changes)}`, new Date(now * 1000)); }

test('valid identity, own-node access and scoped authorization', async () => {
  const principal = await check();
  authorize(principal, 'nodes:write:self', 'punch-1');
  assert.throws(() => authorize(principal, 'nodes:write:self', 'another'), ForbiddenError);
  assert.throws(() => authorize(principal, 'discovery:punch:read'), ForbiddenError);
});
test('forged signatures and unknown signing keys are rejected', async () => {
  await assert.rejects(verify(`Bearer ${await token({}, other.privateKey)}`, new Date(now * 1000)), AuthError);
  await assert.rejects(verify(`Bearer ${await token({}, pair.privateKey, 'unknown')}`, new Date(now * 1000)), AuthError);
});
test('expiry is strict, even within clock tolerance; bad signatures do not disclose expired', async () => {
  await assert.rejects(check({ iat: now - 1000, exp: now }), { code: 'token_expired' });
  await assert.rejects(check({ iat: now - 1000, exp: now - 100 }), { code: 'token_expired' });
  await assert.rejects(verify(`Bearer ${await token({ iat: now - 1000, exp: now - 100 }, other.privateKey)}`, new Date(now * 1000)), { code: 'invalid_token' });
});
test('issuer, audience, identity descriptors, scopes and time ranges are checked', async () => {
  for (const changes of [
    { iss: 'other' }, { aud: 'other' }, { service_type: 'other' }, { scope: ['nodes:read:display'] },
    { scope: ['unknown'] }, { scope: [] }, { sub: '../other' }, { iat: now + 60 },
    { identity_version: 0 }, { public_key: 'invalid' }, { advertise_url: 'quic://user@host:4433' },
    { advertise_url: 'http://host' }, { advertise_url: 'quic://host:4433/path' },
    { exp: now - 1, iat: now + 1 },
  ]) await assert.rejects(check(changes), AuthError);
});
test('platform and track claims have distinct privileges and identity requirements', async () => {
  const { advertise_url: _a, public_key: _p, identity_version: _v, ...shared } = base;
  const jwt = await new SignJWT({ ...shared, service_type: 'platform', scope: ['nodes:read:display'] })
    .setProtectedHeader({ alg: 'EdDSA', typ: 'JWT', kid: 'key-1' }).sign(pair.privateKey);
  const platform = await verify(`Bearer ${jwt}`, new Date(now * 1000));
  authorize(platform, 'nodes:read:display');
  assert.throws(() => authorize(platform, 'nodes:write:self'), ForbiddenError);
  const track = await check({ service_type: 'track', advertise_url: 'https://track.example', public_key: undefined,
    scope: ['nodes:write:self', 'discovery:punch:read'] });
  authorize(track, 'discovery:punch:read');
});
test('no algorithm substitution or malformed authorization', async () => {
  const hs = await new SignJWT(base).setProtectedHeader({ alg: 'HS256', kid: 'key-1', typ: 'JWT' }).sign(new Uint8Array(32));
  for (const header of [undefined, 'Basic abc', 'Bearer invalid', `Bearer ${hs}`]) {
    await assert.rejects(verify(header, new Date(now * 1000)), AuthError);
  }
});
test('trusted key configuration rejects private keys, duplicates and wrong curves', () => {
  for (const keys of [[{ ...jwk, d: 'secret' }], [jwk, jwk], [{ ...jwk, crv: 'X25519' }], []]) {
    assert.throws(() => createVerifier(JSON.stringify({ keys })), RegistryConfigError);
  }
});
test('public key rotation accepts configured keys and rejects removed ones', async () => {
  const next = { ...await exportJWK(other.publicKey), kid: 'key-2' };
  const both = createVerifier(JSON.stringify({ keys: [jwk, next] }));
  const jwt = await token({}, other.privateKey, 'key-2');
  await both(`Bearer ${jwt}`, new Date(now * 1000));
  await assert.rejects(verify(`Bearer ${jwt}`, new Date(now * 1000)), AuthError);
});
test('Hono middleware applies authentication and permissions without leaking credentials', async () => {
  const app = new Hono<RegistryEnv>();
  app.onError((error, c) => {
    if (error instanceof AuthError || error instanceof ForbiddenError || error instanceof RegistryConfigError) {
      return c.json({ code: error.code, detail: error.message }, error.status);
    }
    return c.json({ code: 'internal_error' }, 500);
  });
  app.use('*', authentication());
  app.get('/display', permissionRequired('nodes:read:display'), c => c.json({ id: c.get('principal').sub }));
  const expired = await app.request('/display', { headers: { Authorization: `Bearer ${await token({ exp: 1 })}` } }, { REGISTRY_JWKS: jwks, DB: {} as D1Database });
  assert.equal(expired.status, 401);
  const denied = await app.request('/display', { headers: { Authorization: `Bearer ${await token({ iat: Math.floor(Date.now()/1000), exp: Math.floor(Date.now()/1000)+3600 })}` } }, { REGISTRY_JWKS: jwks, DB: {} as D1Database });
  assert.equal(denied.status, 403);
  assert.deepEqual(await denied.json(), { code: 'forbidden', detail: 'Operation is not permitted' });
});
