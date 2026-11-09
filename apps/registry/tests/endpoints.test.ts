import assert from 'node:assert/strict';
import test from 'node:test';
import { DatabaseSync } from 'node:sqlite';
import { readFileSync } from 'node:fs';
import { managedPunchSchema } from '@q2q/punch-management';
import { punchAssignmentSchema } from '@q2q/punch-discovery';
import { createVerifier, AuthError } from '../src/auth.ts';
import vector from '../../../tests/fixtures/punch/credential.json' with {type:'json'};

test('Registry verifies the shared raw credential and exposes its complete canonical identity',async()=>{
  const verify=createVerifier(JSON.stringify({keys:[{...vector.public_jwk,kid:vector.header.kid}]}));
  const principal=await verify(`Bearer ${vector.token}`,new Date(vector.claims.iat*1000));
  assert.equal(principal.service_type,'punch');
  if(principal.service_type==='punch') {
    const assignment=punchAssignmentSchema.parse({punch_id:principal.sub,endpoints:principal.endpoints,public_key:principal.public_key});
    assert.deepEqual(assignment.endpoints,vector.claims.endpoints);
    assert.equal(principal.identity_version,vector.claims.identity_version);
  }
  const [header,body,signature]=vector.token.split('.');
  const claims=JSON.parse(Buffer.from(body!,'base64url').toString());claims.endpoints.reverse();
  const modified=`${header}.${Buffer.from(JSON.stringify(claims)).toString('base64url')}.${signature}`;
  await assert.rejects(verify(`Bearer ${modified}`,new Date(vector.claims.iat*1000)),AuthError);
});
test('management identity uses the discovery endpoint validator and rejects all obsolete fields',()=>{
  const node={node_id:'test',service_type:'punch',region:null,version:'test',endpoints:[...vector.claims.endpoints].reverse(),
    public_key:vector.claims.public_key,identity_version:2,online:true,accepting_new_connections:true,metrics:null,last_seen:100,expires_at:190,enabled:true,revision:0};
  assert.deepEqual(managedPunchSchema.parse(node).endpoints,vector.claims.endpoints);
  for(const field of ['url','advertise_url','websocket_url']) assert.equal(managedPunchSchema.safeParse({...node,[field]:'quic://old.invalid:4433'}).success,false);
  assert.equal(managedPunchSchema.safeParse({...node,endpoints:[...node.endpoints,node.endpoints[0]]}).success,false);
});
test('fresh Registry SQL separates Punch endpoint arrays from Track HTTPS identity',()=>{
  const db=new DatabaseSync(':memory:');
  try {
    db.exec(readFileSync(new URL('../migrations/0001_nodes.sql',import.meta.url),'utf8'));
    const insert=db.prepare('INSERT INTO nodes VALUES(?,?,?,?,?,?,?,NULL,1,NULL,100,100,190)');
    const key=vector.claims.public_key;
    for(const endpoints of ['null','{}','[]','invalid',JSON.stringify(Array(9).fill('wss://proxy.invalid/'))]) {
      assert.throws(()=>insert.run('bad','punch',1,null,endpoints,key,'test'));
    }
    assert.throws(()=>insert.run('bad','punch',1,'quic://old.invalid:4433',JSON.stringify(vector.claims.endpoints),key,'test'));
    assert.throws(()=>insert.run('bad','track',1,'https://track.invalid',JSON.stringify(vector.claims.endpoints),null,'test'));
    insert.run('punch','punch',1,null,JSON.stringify(vector.claims.endpoints),key,'test');
    insert.run('track','track',1,'https://track.invalid',null,null,'test');
    assert.equal(db.prepare('SELECT count(*) AS n FROM nodes').get()!.n,2);
  } finally {db.close();}
});
