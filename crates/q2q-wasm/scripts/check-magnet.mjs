import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { initSync, SeedBuilder, magnet_seed, parse_magnet, seed_info_b64, seed_magnet } from '../pkg/q2q_wasm.js';

initSync({ module: readFileSync(new URL('../pkg/q2q_wasm_bg.wasm', import.meta.url)) });
const hash64 = 'a'.repeat(64);

// Malformed links throw stable machine codes instead of panicking.
for (const [link, code] of [
  ['http://example.com/file', 'not_magnet'],
  ['magnet:?dn=name', 'missing_xt'],
  ['magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567', 'wrong_namespace'],
  [`magnet:?xt=urn:sha256:${'z'.repeat(64)}`, 'bad_hash'],
  [`magnet:?xt=urn:sha256:${'a'.repeat(63)}`, 'bad_hash'],
  ['magnet:?xt=urn%zz', 'bad_encoding'],
  ['magnet:?xt=urn:sha256:%zz', 'bad_encoding'],
  ['magnet:?xt=urn:sha256:' + hash64 + '#frag', 'bad_encoding'],
]) assert.throws(() => parse_magnet(link), { message: code }, link);
assert.throws(
  () => parse_magnet(`magnet:?xt=urn:sha256:${hash64}&xt=urn:sha256:${'b'.repeat(64)}`),
  { message: 'conflicting_xt' });
assert.throws(
  () => parse_magnet(`magnet:?xt=urn:sha256:${hash64}&dn=${'x'.repeat(17 * 1024)}`),
  { message: 'too_long' });
console.log('PASS malformed magnets throw stable codes and never panic');

// Roundtrip: seed -> link -> parse; tr hints normalize https and deduplicate.
const content = Buffer.alloc(300 * 1024 + 7);
for (let i = 0; i < content.length; i++) content[i] = (i * 17 + (i >> 18)) % 251;
const builder = new SeedBuilder(content.length);
for (let offset = 0; offset < content.length; offset += 999983) builder.update(content.subarray(offset, offset + 999983));
const prepared = builder.finish('magnet-fixture.bin');
const seedBytes = prepared.seed_bytes();
const infoHash = Buffer.from(prepared.info_hash());
const link = seed_magnet(seedBytes);
assert.equal(typeof link, 'string');
assert.ok(link.startsWith(`magnet:?xt=urn:sha256:${infoHash.toString('hex')}`));
assert.ok(link.includes('&dn=magnet-fixture.bin'));
const parsed = JSON.parse(parse_magnet(link));
assert.equal(parsed.infoHash, infoHash.toString('hex'));
assert.equal(parsed.displayName, 'magnet-fixture.bin');
assert.deepEqual(parsed.trackers, []);

const withTrack = seed_magnet(seedBytes) + '&tr=track.example.com&tr=track.example.com&tr=' + encodeURIComponent('http://localhost:8080');
const tracked = JSON.parse(parse_magnet(withTrack));
assert.deepEqual(tracked.trackers, ['https://track.example.com', 'http://localhost:8080']);
// Invalid tr hints are ignored without changing the identity.
const badTrack = JSON.parse(parse_magnet(`${link}&tr=${encodeURIComponent('ftp://x')}&tr=${encodeURIComponent('u:p@h')}`));
assert.deepEqual(badTrack.trackers, []);
assert.equal(badTrack.infoHash, parsed.infoHash);
// The same xt twice is not a conflict.
assert.equal(JSON.parse(parse_magnet(`${link}&xt=urn:sha256:${infoHash.toString('hex')}`)).infoHash, parsed.infoHash);
console.log('PASS magnet roundtrip, dn, tr normalization and dedupe');

// Metadata verification: identity binds to the raw canonical info bytes.
const infoB64 = seed_info_b64(seedBytes);
const minimal = Buffer.from(magnet_seed(infoB64, infoHash));
const minimalInfo = JSON.parse(parse_magnet(seed_magnet(minimal)));
assert.equal(minimalInfo.infoHash, infoHash.toString('hex'));
// The minimal seed re-encodes and stays self-consistent with a fresh info extraction.
assert.equal(seed_info_b64(minimal), infoB64);

const corrupt = Buffer.from(infoB64, 'base64');
// Corrupt inside the name string: bencode stays valid, but the digest changes.
corrupt[corrupt.length - 3] ^= 1;
assert.throws(() => magnet_seed(corrupt.toString('base64'), infoHash), { message: 'info_hash_mismatch' });
assert.throws(() => magnet_seed(infoB64, Buffer.alloc(32)), { message: 'info_hash_mismatch' });
assert.throws(() => magnet_seed(infoB64, Buffer.alloc(31)), { message: 'bad_hash' });
assert.throws(() => magnet_seed(Buffer.alloc(2).toString('base64') + '!', infoHash), { message: 'malformed_metadata' });
assert.throws(() => magnet_seed(Buffer.alloc(8193).toString('base64'), infoHash), { message: 'info_too_large' });
// A non-seed payload fails closed.
assert.throws(() => seed_info_b64(Buffer.from('not a seed')), Error);
assert.throws(() => seed_magnet(Buffer.from('not a seed')), Error);
console.log('PASS metadata verification rejects wrong hash, corruption and oversized info');

// The minimal seed parses as a valid peer-only seed via the shared decoder.
const decoded = JSON.parse((await import('../pkg/q2q_wasm.js')).decode_seed(minimal));
assert.equal(decoded.infoHash, infoHash.toString('hex'));
assert.equal(decoded.name, 'magnet-fixture.bin');
assert.equal(Number(decoded.blockCount), Math.ceil(content.length / (256 * 1024)));
assert.deepEqual(decoded.sources, []);
assert.deepEqual(decoded.proofSources, []);
assert.deepEqual(decoded.tracks, []);
assert.equal(decoded.inlineProof, false);
console.log('PASS minimal magnet seed is a valid peer-only seed');

prepared.free();
