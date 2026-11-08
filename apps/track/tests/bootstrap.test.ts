import assert from 'node:assert/strict';
import { test } from 'node:test';
import { runtime } from './runtime-fixture.ts';
import { schemaQuery, validateSchema } from '../../registry/scripts/check-schema.ts';
import vectors from '../../../tests/fixtures/track/v1.json' with {type:'json'};

test('fresh Registry D1 and Track namespaces bootstrap the list protocol without old state', {timeout:30000}, async()=>{
  const punch={punch_id:'fixture-punch',endpoints:['quic://punch.invalid:443','ws://127.0.0.1:8080/signal?region=local','wss://proxy.invalid/signal?region=hk'],public_key:Buffer.alloc(32,9).toString('base64')};
  const mf=await runtime('5000',punch);
  try {
    const db=await mf.getD1Database('DB','sdk-registry');
    validateSchema((await db.prepare(schemaQuery).all()).results);
    assert.equal(await db.prepare('SELECT count(*) AS n FROM nodes').first('n'),1);
    assert.equal(await db.prepare('SELECT count(*) AS n FROM punch_operation').first('n'),0);
    const peer=JSON.parse(vectors.tickets[0]!.body_json).iss as string;
    const empty=await mf.dispatchFetch('https://track.invalid/inspect',{method:'POST',body:JSON.stringify({peer_id:peer})});
    assert.deepEqual(await empty.json(),{session:null,pending:[],alarm:null,tables:0});
    const status=await mf.dispatchFetch('https://track.invalid/discovery-test?operation=status');
    assert.deepEqual(await status.json(),{alarm:null});
    const snapshot=await mf.dispatchFetch('https://track.invalid/discovery-test');
    assert.equal(snapshot.status,200);
    const initial=await snapshot.json() as {nodes:{endpoints:string[]}[]};
    assert.deepEqual(initial.nodes[0]!.endpoints,punch.endpoints);
    const identities=await (await mf.dispatchFetch('https://track.invalid/discovery-test?operation=identities')).json() as {version:unknown;columns:string[];rows:unknown[]};
    assert.deepEqual(identities.version,{version:2});
    assert.deepEqual(identities.columns,['id','version','endpoints_json','public_key']);
    assert.equal(identities.rows.length,1);
    const accepted=await mf.dispatchFetch('https://track.invalid/endpoint-checks?scenario=sessions');
    assert.equal(accepted.status,200,await accepted.clone().text());
    const before=await (await mf.dispatchFetch('https://track.invalid/discovery-test?operation=identities')).json();
    await mf.unsafeEvictDurableObject('track-state','TestDirectory',{name:'punch-discovery:v1'});
    assert.deepEqual(await (await mf.dispatchFetch('https://track.invalid/discovery-test?operation=identities')).json(),before);
    // Another namespace remains empty: a reset must cover every state binding,
    // not assume clearing Registry automatically migrates a Track deployment.
    const isolated=await mf.getWorker('track-isolated');
    assert.deepEqual(await (await isolated.fetch('https://track.invalid/inspect',{method:'POST',body:JSON.stringify({peer_id:peer})})).json(),{session:null,pending:[],alarm:null,tables:0});
  } finally {await mf.dispose();}
});
