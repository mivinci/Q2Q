import { test } from 'node:test';
import assert from 'node:assert/strict';
import { acceptsNewPeer, discoveryPageSchema } from '@q2q/punch-discovery';
const metrics = {quic_connections:10,quic_connection_capacity:10,websocket_connections:10,
  websocket_connection_capacity:10,live_bindings:0,signalling_sessions:0,signalling_session_capacity:null};
test('assignment requires a matching endpoint with capacity; WS/WSS share one budget',()=>{
  const node={endpoints:['quic://punch.invalid:4433','wss://proxy.invalid/ws'],accepting_new_connections:true,metrics};
  assert.equal(acceptsNewPeer(node,['quic','wss']),false);
  assert.equal(acceptsNewPeer({...node,metrics:{...metrics,websocket_connections:9}},['quic']),false);
  assert.equal(acceptsNewPeer({...node,metrics:{...metrics,websocket_connections:9}},['wss']),true);
  assert.equal(acceptsNewPeer({...node,metrics:null},['ws']),false);
  assert.equal(acceptsNewPeer({...node,metrics:null},['wss']),true);
  assert.equal(acceptsNewPeer({...node,accepting_new_connections:false,metrics:null},['wss']),false);
});
test('RPC rejects old fields, excessive deadlines and oversized pages; canonicalizes endpoints',()=>{
  const node={punch_id:'punch',endpoints:['wss://proxy.invalid/ws?region=hk','quic://punch.invalid:4433'],public_key:'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=',
    identity_version:1,region:null,accepting_new_connections:true,metrics:null,last_seen:100,expires_at:190};
  const page={version:1,environment:'test',server_time:100,expires_at:160,nodes:[node],next_cursor:null};
  assert.deepEqual(discoveryPageSchema.parse(page).nodes[0]!.endpoints,[...node.endpoints].sort());
  assert.equal(discoveryPageSchema.safeParse({...page,expires_at:161}).success,false);
  assert.equal(discoveryPageSchema.safeParse({...page,nodes:Array(201).fill(node)}).success,false);
  for(const field of ['url','advertise_url','websocket_url']) assert.equal(discoveryPageSchema.safeParse({...page,nodes:[{...node,[field]:'quic://old.invalid:4433'}]}).success,false);
  assert.equal(discoveryPageSchema.safeParse({...page,nodes:[{...node,endpoints:['ws://proxy.invalid']}]}).success,false);
});
