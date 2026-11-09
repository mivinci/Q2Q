import assert from 'node:assert/strict';
import { ControlIdentity, validate_peer_control, encode_peer_piece, peer_piece_header, PeerBlockAssembly, decode_peer_control, peer_bitfield, encode_peer_bitfield } from '../pkg/q2q_wasm.js';
import { createPrivateKey, createPublicKey, verify } from 'node:crypto';
const a=new ControlIdentity(new Uint8Array(32).fill(7)),b=new ControlIdentity(new Uint8Array(32).fill(8));
const pub=id=>Buffer.from(JSON.parse(id.info()).public_key,'base64');
const sdp=value=>`v=0\r\na=fingerprint:sha-256 ${Array(32).fill(value).join(':')}\r\n`;
const hash=new Uint8Array(32).fill(3),n1=new Uint8Array(32).fill(1),n2=new Uint8Array(32).fill(2);
const u64=Buffer.alloc(8);u64.writeBigUInt64BE(4n);
const privateKey=createPrivateKey({key:Buffer.concat([Buffer.from('302e020100300506032b657004220420','hex'),new Uint8Array(32).fill(7)]),format:'der',type:'pkcs8'});
const first=a.begin_peer(pub(b),true,hash,'test',n1,sdp('AA'),sdp('BB'));
const second=b.begin_peer(pub(a),false,hash,'test',n2,sdp('BB'),sdp('AA'));
const hello1=first.hello(),hello2=second.hello();
const proof1=first.receive(hello2),proof2=second.receive(hello1);
const transcript=Buffer.concat([Buffer.from('q2q/peer-identity/v1\0'),Buffer.from([0]),pub(a),pub(b),hash,u64,Buffer.from('test'),n1,n2,Buffer.alloc(32,0xaa),Buffer.alloc(32,0xbb)]);
assert.ok(verify(null,transcript,createPublicKey(privateKey),Buffer.from(JSON.parse(proof1).signature,'base64')));
assert.equal(first.receive(proof2),undefined);assert.equal(second.receive(proof1),undefined);
assert.ok(first.authenticated()&&second.authenticated());first.free();second.free();
for(const changed of ['fingerprint','hash','connection','role','key']){
  const left=a.begin_peer(pub(b),true,hash,'test',n1,sdp('AA'),sdp('BB'));
  const right=b.begin_peer(changed==='key'?pub(b):pub(a),changed==='role',changed==='hash'?new Uint8Array(32).fill(4):hash,changed==='connection'?'other':'test',n2,sdp('BB'),sdp(changed==='fingerprint'?'CC':'AA'));
  const x=left.hello(),y=right.hello();const p=right.receive(x);left.receive(y);
  assert.throws(()=>left.receive(p));assert.equal(left.authenticated(),false);assert.throws(()=>left.receive(p));left.free();right.free();
}
const replay=a.begin_peer(pub(b),true,hash,'test',new Uint8Array(32).fill(99),sdp('AA'),sdp('BB'));
replay.receive(hello2);assert.throws(()=>replay.receive(proof2));replay.free();
for(const raw of ['{"type":"identity_proof","v":1,"signature":"AA=="}','{"type":"identity_hello","v":1,"nonce":"AA==","nonce":"AA=="}',' '.repeat(513)]){
  const auth=a.begin_peer(pub(b),true,hash,'test',n1,sdp('AA'),sdp('BB'));assert.throws(()=>auth.receive(raw));auth.free();
}
for(const bad of ['v=0',sdp('AA').replace('sha-256','sha-1'),sdp('AA')+sdp('BB')])assert.throws(()=>a.begin_peer(pub(b),true,hash,'test',n1,bad,sdp('BB')));
const control='{"v":1,"message":{"type":"have","block_index":18446744073709551615}}';validate_peer_control(control);
assert.throws(()=>validate_peer_control(control.replace('"v":1','"v":2')));
const piece=encode_peer_piece((1n<<64n)-1n,1024,new Uint8Array(1024).fill(9));
assert.equal(piece.length,1041);assert.deepEqual(JSON.parse(peer_piece_header(piece)),{block_index:'18446744073709551615',piece_offset:1024});
assert.throws(()=>encode_peer_piece(0n,1,new Uint8Array(1)));assert.throws(()=>peer_piece_header(new Uint8Array(1042)));
a.free();b.free();console.log('PASS channel-bound identity, independent Node transcript proof, replay/mutation rejection and exact peer wire bounds');

const assembly=new PeerBlockAssembly(2051);
assembly.insert(2048,new Uint8Array([3,3,3]));assembly.insert(0,new Uint8Array(1024).fill(1));
assert.equal(assembly.missing(),'[[1024,1024]]');assert.throws(()=>assembly.bytes());
assert.equal(assembly.insert(0,new Uint8Array(1024).fill(1)),false);
assert.throws(()=>assembly.insert(0,new Uint8Array(1024).fill(9)));
assert.throws(()=>assembly.insert(1025,new Uint8Array(1024)));
assembly.insert(1024,new Uint8Array(1024).fill(2));assert.equal(assembly.complete(),true);assert.equal(assembly.bytes().length,2051);assembly.free();
assert.throws(()=>new PeerBlockAssembly(4194305));
assert.equal(JSON.parse(decode_peer_control(control)).block_index,'18446744073709551615');
assert.deepEqual(Array.from(peer_bitfield('{"v":1,"message":{"type":"bitfield","bits":"8A=="}}',4)),[15]);
assert.equal(JSON.parse(encode_peer_bitfield(new Uint8Array([15]),4)).message.bits,'8A==');
assert.throws(()=>peer_bitfield('{"v":1,"message":{"type":"bitfield","bits":"AQ=="}}',4));
assert.throws(()=>encode_peer_bitfield(new Uint8Array([240]),4));
const issuer=new ControlIdentity(new Uint8Array(32).fill(7)),responder=new ControlIdentity(new Uint8Array(32).fill(8));
const issuerInfo=JSON.parse(issuer.info());const ticket=responder.connect_ticket('punch',issuerInfo.peer_id,hash,'test',1000000n);
issuer.verify_accept(ticket,pub(responder),'test','punch',hash,1000000n);
for(const field of ['connection','punch','hash','key','expired'])assert.throws(()=>issuer.verify_accept(ticket,field==='key'?pub(issuer):pub(responder),field==='connection'?'wrong':'test',field==='punch'?'wrong':'punch',field==='hash'?new Uint8Array(32):hash,field==='expired'?1001000n:1000000n));
issuer.free();responder.free();console.log('PASS shared assembly repair/conflicts, wire/checkpoint bit order, exact peer indices and accept-ticket binding');
