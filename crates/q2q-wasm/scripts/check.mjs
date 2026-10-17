import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { initSync, SeedBuilder } from '../pkg/q2q_wasm.js';

const wasm = initSync({ module: readFileSync(new URL('../pkg/q2q_wasm_bg.wasm', import.meta.url)) });
const blockSize = 256 * 1024;
const hash = (...bytes) => {
  const result = createHash('sha256');
  for (const chunk of bytes) result.update(chunk);
  return result.digest();
};

// Independent reference decoder; preserve each info dictionary's exact bytes.
function decode(bytes) {
  let offset = 0;
  let infoBytes;
  function value() {
    const start = offset;
    const tag = String.fromCharCode(bytes[offset++]);
    if (tag === 'i') {
      while (bytes[offset] !== 101) offset++;
      return Number(bytes.subarray(start + 1, offset++).toString());
    }
    if (tag === 'd') {
      const object = {};
      while (bytes[offset] !== 101) {
        const key = value().toString();
        const begin = offset;
        object[key] = value();
        if (key === 'info') infoBytes = bytes.subarray(begin, offset);
      }
      offset++;
      return object;
    }
    if (tag === 'l') {
      const items = [];
      while (bytes[offset] !== 101) items.push(value());
      offset++;
      return items;
    }
    while (bytes[offset] !== 58) offset++;
    const length = Number(bytes.subarray(start, offset).toString());
    offset++;
    const data = bytes.subarray(offset, offset + length);
    offset += length;
    return data;
  }
  return { seed: value(), get infoBytes() { return infoBytes; } };
}

for (const size of [0, 1, blockSize, 3 * blockSize + 17]) {
  const bytes = Buffer.alloc(size, 42);
  const builder = new SeedBuilder(size);
  for (let offset = 0; offset < size; offset += 997) builder.update(bytes.subarray(offset, offset + 997));
  const prepared = builder.finish('fixture.bin');
  const leaves = [];
  for (let offset = 0; offset < size; offset += blockSize) leaves.push(hash(Buffer.from([0]), bytes.subarray(offset, offset + blockSize)));
  assert.deepEqual(Buffer.from(prepared.proof_bytes()), Buffer.concat(leaves));
  let layer = leaves;
  while (layer.length > 1) {
    const next = [];
    for (let i = 0; i < layer.length; i += 2) next.push(layer[i + 1] ? hash(Buffer.from([1]), layer[i], layer[i + 1]) : layer[i]);
    layer = next;
  }
  const decoded = decode(Buffer.from(prepared.seed_bytes()));
  assert.deepEqual(decoded.seed.info.root, layer[0] ?? Buffer.alloc(32));
  assert.equal(decoded.seed.info.length, size);
  assert.equal(decoded.seed.info.name.toString(), 'fixture.bin');
  assert.deepEqual(Buffer.from(prepared.info_hash()), hash(decoded.infoBytes));
  assert.equal(decoded.seed.leaf_hashes, undefined);
  prepared.free();
}
for (const length of [-1, 0.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1]) assert.throws(() => new SeedBuilder(length));
const incomplete = new SeedBuilder(1);
assert.throws(() => incomplete.finish('incomplete'));
const overflow = new SeedBuilder(1);
assert.throws(() => overflow.update(new Uint8Array(2)));
overflow.update(new Uint8Array(1));
overflow.finish('valid').free();
console.log('PASS Wasm chunking, proof bytes, root, seed identity and length errors');

const inlineBuilder = new SeedBuilder(3);
inlineBuilder.update(Buffer.from('abc'));
const inlineResult = inlineBuilder.finish_inline('abc.txt');
const inlineSeed = decode(Buffer.from(inlineResult.seed_bytes()));
assert.deepEqual(inlineSeed.seed.leaf_hashes, Buffer.from(inlineResult.proof_bytes()));
assert.deepEqual(Buffer.from(inlineResult.info_hash()), hash(inlineSeed.infoBytes));
inlineResult.free();
console.log('PASS standalone seed with inline leaf hashes');

const chunk = new Uint8Array(1024 * 1024);
for (const mib of process.argv.slice(2).length ? process.argv.slice(2).map(Number) : [64, 256]) {
  assert(Number.isSafeInteger(mib) && mib > 0 && mib <= 8192);
  const start = performance.now();
  const builder = new SeedBuilder(mib * 1024 * 1024);
  for (let i = 0; i < mib; i++) builder.update(chunk);
  const result = builder.finish('benchmark.bin');
  const elapsedMs = Math.round(performance.now() - start);
  const proofBytes = result.proof_bytes().byteLength;
  assert.equal(proofBytes, mib * 1024 * 1024 / blockSize * 32);
  console.log(JSON.stringify({ mib, elapsedMs, wasmMemoryMiB: wasm.memory.buffer.byteLength / 1024 ** 2, proofBytes }));
  result.free();
}
