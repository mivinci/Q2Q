import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {ControlIdentity,SeedBuilder,UploadProofs,PeerProofVerifier} from '../pkg/q2q_wasm.js';
const a=new ControlIdentity(new Uint8Array(32).fill(21)),b=new ControlIdentity(new Uint8Array(32).fill(22));
const target=JSON.parse(b.info()),source=JSON.parse(a.info()),hash=new Uint8Array(32).fill(7);
const raw=a.connect_ticket('punch-test',target.peer_id,hash,'connection-test',1000000n);
assert.deepEqual(JSON.parse(b.inspect_connect(raw,'punch-test','connection-test',1000001n)),{peer_id:source.peer_id,public_key:source.public_key,info_hash:'07'.repeat(32)});
for(const work of [()=>a.inspect_connect(raw,'punch-test','connection-test',1000001n),()=>b.inspect_connect(raw,'other','connection-test',1000001n),()=>b.inspect_connect(raw,'punch-test','other',1000001n),()=>b.inspect_connect(raw,'punch-test','connection-test',1000061n)])assert.throws(work);
const forged=JSON.parse(raw);forged.ih=Buffer.alloc(32,8).toString('base64');assert.throws(()=>b.inspect_connect(JSON.stringify(forged),'punch-test','connection-test',1000001n));
a.free();b.free();
const bytes=Buffer.alloc(3*262144+17);for(let i=0;i<bytes.length;i++)bytes[i]=(i*31+Math.floor(i/262144))%251;
const builder=new SeedBuilder(bytes.length);builder.update(bytes);const prepared=builder.finish_inline('upload.bin');
const proofs=new UploadProofs(prepared.seed_bytes()),sparse=new PeerProofVerifier(prepared.seed_bytes());
for(let index=0;index<4;index++){
 const block=bytes.subarray(index*262144,Math.min(bytes.length,(index+1)*262144));
 const frame=proofs.proof(index),decoded=JSON.parse(frame);assert.equal(decoded.message.block_index,index);
 const leaf=createHash('sha256').update(Buffer.from([0])).update(block).digest();
 assert.deepEqual(Buffer.from(decoded.message.proof.leaf_hashes[0]),leaf);
 assert.ok(proofs.verify(index,block));const witness=sparse.witness(index,block,frame);assert.ok(sparse.verify(index,block,witness));
 const relayed=sparse.proof_frame(index,block,witness);assert.ok(sparse.verify(index,block,sparse.witness(index,block,relayed)));
 const corruptWitness=new Uint8Array(witness);corruptWitness[corruptWitness.length-1]^=1;
 assert.throws(()=>sparse.proof_frame(index,block,corruptWitness));
 const broken=Buffer.from(block);broken[0]^=1;assert.equal(proofs.verify(index,broken),false);
}
assert.throws(()=>proofs.proof(4));proofs.free();sparse.free();prepared.free();
console.log('PASS incoming ticket target/audience/connection/signature and native upload proof generation');
