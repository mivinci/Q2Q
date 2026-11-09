import assert from 'node:assert/strict';
import { createHmac, createPrivateKey, createPublicKey, verify } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { ControlIdentity, punch_binding_auth, decode_punch_frame } from '../pkg/q2q_wasm.js';
// Called after the content check has initialized the same wasm-bindgen module.
const vectors=JSON.parse(readFileSync(new URL('../../../tests/fixtures/track/v1.json',import.meta.url)));
const zero=new ControlIdentity(new Uint8Array(32));
assert.deepEqual(JSON.parse(zero.info()),vectors.identities[0]);zero.free();
const seed=new Uint8Array(32).fill(7);const identity=new ControlIdentity(seed);
const info=JSON.parse(identity.info());
const privateKey=createPrivateKey({key:Buffer.concat([Buffer.from('302e020100300506032b657004220420','hex'),seed]),format:'der',type:'pkcs8'});
const pub=createPublicKey(privateKey);
assert.equal(Buffer.from(pub.export({format:'der',type:'spki'})).subarray(-32).toString('base64'),info.public_key);
const sessions=vectors.sessions.filter(value=>value.peer_id===info.peer_id&&value.error===null);
assert.ok(sessions.length>0,'native session fixtures must actually be exercised');
for(const vector of sessions) {
  const json=identity.track_auth(Buffer.from(vector.key_base64,'base64'),BigInt(vector.seq),BigInt(vector.timestamp));
  const result=JSON.parse(json);assert.equal(result.mac,vector.mac);
  assert.ok(json.includes(`"seq":${vector.seq}`),'u64 wire tokens are exact');
}
const join=JSON.parse(identity.join('punch-test','nonce-test',1000000n));
assert.ok(verify(null,Buffer.from(`join${join.join_token}${join.timestamp}`),pub,Buffer.from(join.signature,'base64')));
const u64=value=>{const b=Buffer.alloc(8);b.writeBigUInt64BE(value);return b;};
const bindingKey=Buffer.alloc(32,11),seq=(1n<<64n)-1n;
for(const exit of [false,true]) {
  const json=punch_binding_auth(bindingKey,'binding-test',seq,1000000n,3n,exit);
  const result=JSON.parse(json);
  const payload=Buffer.concat([...(exit?[Buffer.from('punch.exit')]:[]),Buffer.from('binding-test'),u64(seq),u64(1000000n),u64(3n)]);
  assert.equal(result.mac,createHmac('sha256',bindingKey).update(payload).digest('hex'));
  assert.ok(json.includes(`"seq":${seq}`));
}
assert.throws(()=>new ControlIdentity(new Uint8Array(31)));
assert.throws(()=>identity.join('bad/punch','nonce',1000000n));
assert.throws(()=>identity.announce((1n<<64n)-1n));
const good=Buffer.from('{"type":"response","payload":{"v":1,"id":"request","result":{"ok":true}}}');
assert.equal(JSON.parse(decode_punch_frame(good)).payload.id,'request');
for(const raw of ['{"type":"response","payload":{"v":1,"v":1,"id":"x","result":{}}}', '{"type":"response","payload":{"v":2,"id":"x","result":{}}}'])assert.throws(()=>decode_punch_frame(Buffer.from(raw)));
assert.throws(()=>decode_punch_frame(new Uint8Array(65537)));
identity.free();
console.log('PASS control identity, native Track vectors, independent Node join signatures, u64 Punch heartbeat/exit MACs and binary frame validation');
