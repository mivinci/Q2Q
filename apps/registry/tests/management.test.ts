import assert from 'node:assert/strict';
import {test} from 'node:test';
import {DatabaseSync} from 'node:sqlite';
import {readFileSync,readdirSync} from 'node:fs';
import {setPolicy,getOperation,getPunch,listPunches} from '../src/management.ts';
import {discoverPage} from '../src/nodes.ts';
function fixture(){
 const sql=new DatabaseSync(':memory:');const directory=new URL('../migrations/',import.meta.url);
 for(const name of readdirSync(directory).sort())if(name.endsWith('.sql'))sql.exec(readFileSync(new URL(name,directory),'utf8'));
 const now=Math.floor(Date.now()/1000);
 sql.prepare("INSERT INTO nodes VALUES('punch','punch',1,'quic://punch.example:4433',?,'test','hk',1,NULL,?,?,?)").run('AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=',now,now,now+90);
 sql.prepare("INSERT INTO nodes VALUES('legacy-track','track',1,'https://track.example',NULL,'test',NULL,1,NULL,?,?,?)").run(now,now,now+90);
 let tail=Promise.resolve();
 const db={prepare(query:string){let args:(string|number|null)[]=[];return{bind(...values:(string|number|null)[]){args=values;return this;},async first(){return sql.prepare(query).get(...args)??null;},async all(){return{success:true,results:sql.prepare(query).all(...args)};}};},batch(statements:{all:()=>Promise<unknown>}[]){const job=tail.then(async()=>{sql.exec('BEGIN');try{const results=[];for(const statement of statements)results.push(await statement.all());sql.exec('COMMIT');return results;}catch(cause){sql.exec('ROLLBACK');throw cause;}});tail=job.then(()=>{},()=>{});return job;}} as unknown as D1Database;
 return{sql,db,now};
}
const input=(revision=0,enabled=false)=>({requestId:crypto.randomUUID(),nodeId:'punch',enabled,revision,reason:'Drain this node',actorId:'operator',actorEmail:'operator@example.test'});
test('stop and resume affect discovery without changing the heartbeat',async()=>{
 const{sql,db}=fixture();try{
  assert.equal((await discoverPage(db,{limit:25})).nodes.length,1);
  const before=sql.prepare("SELECT * FROM nodes WHERE node_id='punch'").get();
  const result=await setPolicy(db,input());assert.equal(result.ok,true);
  assert.equal((await discoverPage(db,{limit:25})).nodes[0].accepting_new_connections,false);
  assert.deepEqual(sql.prepare("SELECT * FROM nodes WHERE node_id='punch'").get(),before);
  assert.equal((await getPunch(db,{nodeId:'punch'}))?.enabled,false);
  const page=await listPunches(db,{limit:25});assert.equal(page.nodes.length,1);assert.equal(page.nodes[0].online,true);
  assert.equal((await setPolicy(db,input(1,true))).ok,true);
  assert.equal((await discoverPage(db,{limit:25})).nodes.length,1);
 }finally{sql.close();}
});
test('expiry and renewed heartbeat do not erase management intent',async()=>{
 const{sql,db,now}=fixture();try{
  await setPolicy(db,input());
  sql.prepare("UPDATE nodes SET created_at=?,last_seen=?,expires_at=? WHERE node_id='punch'").run(now-1000,now-100,now-10);
  assert.equal((await getPunch(db,{nodeId:'punch'}))?.enabled,false);
  sql.prepare("UPDATE nodes SET last_seen=?,expires_at=? WHERE node_id='punch'").run(now,now+90);
  assert.equal((await discoverPage(db,{limit:25})).nodes[0].accepting_new_connections,false);
 }finally{sql.close();}
});
test('retries are idempotent and stale or conflicting requests are rejected',async()=>{
 const{sql,db}=fixture();try{
  const value=input();assert.equal((await setPolicy(db,value)).ok,true);
  assert.equal((await setPolicy(db,value)).ok,true);
  assert.deepEqual(await setPolicy(db,{...value,enabled:true}),{ok:false,code:'request_conflict'});
  assert.deepEqual(await setPolicy(db,input()),{ok:false,code:'conflict'});
  assert.equal(sql.prepare('SELECT COUNT(*) AS count FROM punch_operation').get()!.count,1);
  const operation=await getOperation(db,{requestId:value.requestId});assert.equal(operation?.before.enabled,true);assert.equal(operation?.after.enabled,false);
 }finally{sql.close();}
});
test('concurrent updates serialize and legacy Track cannot be managed',async()=>{
 const{sql,db}=fixture();try{
  const values=await Promise.all([setPolicy(db,input()),setPolicy(db,input())]);assert.equal(values.filter(value=>value.ok).length,1);
  assert.deepEqual(await setPolicy(db,{...input(),nodeId:'legacy-track'}),{ok:false,code:'not_found'});
  assert.equal((await listPunches(db,{limit:25})).nodes.length,1);
 }finally{sql.close();}
});
test('policy failure rolls back its audit record',async()=>{
 const{sql,db}=fixture();try{
  sql.exec("CREATE TRIGGER fail_policy BEFORE INSERT ON punch_policy BEGIN SELECT RAISE(ABORT,'test failure'); END;");
  await assert.rejects(setPolicy(db,input()));
  assert.equal(sql.prepare('SELECT COUNT(*) AS count FROM punch_operation').get()!.count,0);
  assert.equal((await discoverPage(db,{limit:25})).nodes.length,1);
 }finally{sql.close();}
});
