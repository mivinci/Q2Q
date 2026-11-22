import {test} from 'node:test';
import assert from 'node:assert/strict';
import {Q2QError} from '../src/error.ts';
import type {ConnectionDiagnostic} from '../src/error.ts';

test('connection diagnostics copy only bounded enum states and counts',()=>{
 const input={stage:'rtc-connect',connectionState:'connecting',iceConnectionState:'checking',localCandidates:2,remoteCandidates:3,
  sdp:'private-sdp',url:'https://private.invalid',credential:'private-credential',peerId:'private-peer'};
 const error=new Q2QError('CONTROL_TIMEOUT',true,input as ConnectionDiagnostic);
 input.connectionState='closed';input.localCandidates=99;
 assert.deepEqual(error.diagnostic,{stage:'rtc-connect',connectionState:'connecting',iceConnectionState:'checking',localCandidates:2,remoteCandidates:3});
 assert.ok(Object.isFrozen(error.diagnostic));assert.equal(error.code,'CONTROL_TIMEOUT');assert.equal(error.retryable,true);
 assert.ok(!JSON.stringify(error).includes('private'));
});
test('invalid diagnostic values cannot smuggle host messages or unbounded data',()=>{
 const input={stage:'rtc-candidate',connectionState:'token=secret',controlState:'secret',localCandidates:Infinity,remoteCandidates:-1};
 assert.deepEqual(new Q2QError('PEER_PROTOCOL_ERROR',false,input as unknown as ConnectionDiagnostic).diagnostic,{stage:'rtc-candidate'});
 assert.equal(new Q2QError('PEER_CLOSED',false,{stage:'private-sdp'} as unknown as ConnectionDiagnostic).diagnostic,undefined);
 assert.equal(new Q2QError('PEER_CLOSED').diagnostic,undefined);
});
