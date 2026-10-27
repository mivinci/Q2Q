import assert from 'node:assert/strict';
import { test } from 'node:test';
import { DatabaseSync } from 'node:sqlite';
import { readFileSync, readdirSync } from 'node:fs';
import { listRecords, overview, details, plans } from '../src/lib/server/queries.ts';

function fixture(beforeCatalog?: (sql: DatabaseSync) => void) {
 const sql=new DatabaseSync(':memory:');
 const migrations=new URL('../../platform/migrations/',import.meta.url);
 for(const name of readdirSync(migrations).sort()) if(name.endsWith('.sql')) { if(name === '0017_plan_catalog.sql') beforeCatalog?.(sql); sql.exec(readFileSync(new URL(name,migrations),'utf8')); }
 const db={prepare(query:string){let args:(string|number|null)[]=[];return {
  bind(...values:(string|number|null)[]){args=values;return this;},
  async all(){return {results:sql.prepare(query).all(...args),success:true};},
  async first(){return sql.prepare(query).get(...args)??null;}
 };},async batch(statements:{all:()=>Promise<unknown>}[]){return Promise.all(statements.map(statement=>statement.all()));}} as unknown as D1Database;
 const insert=sql.prepare('INSERT INTO user (id,name,email,createdAt,updatedAt) VALUES (?,?,?,0,0)');
 for(let i=0;i<27;i++){const id=`user-${String(i).padStart(2,'0')}`;insert.run(id,`Person ${i}`,`${id}@example.test`);}
 sql.exec(`INSERT INTO project (id,name,createdAt) VALUES ('project','Example',0);
 INSERT INTO project_member (projectId,userId,role,createdAt) VALUES ('project','user-00','owner',0);
 INSERT INTO project_subscription (projectId,planId,status,provider,createdAt,updatedAt) VALUES ('project','team','cancelled','local',0,0);
 INSERT INTO account (id,accountId,providerId,userId,accessToken,createdAt,updatedAt) VALUES ('account','external','github','user-00','never-expose-this-token',0,0);`);
 return {sql,db};
}
test('identity queries paginate, search literally and never expose OAuth credentials',async()=>{
 const {sql,db}=fixture();try{
  const first=await listRecords(db,'users',new URL('https://admin.test/users'));
  assert.equal(first.rows.length,25);assert.equal(first.hasNext,true);
  assert.deepEqual(Object.keys(first.rows[0]).sort(),['accountStatus','createdAt','email','id','name']);
  const second=await listRecords(db,'users',new URL('https://admin.test/users?page=2'));
  assert.equal(second.rows.length,2);assert.equal(second.hasNext,false);
  assert.ok(!first.rows.some(row=>row.id===second.rows[0].id));
  assert.equal((await listRecords(db,'users',new URL('https://admin.test/users?q=user-00'))).rows.length,1);
  assert.equal((await listRecords(db,'users',new URL('https://admin.test/users?q=%27%20OR%201%3D1'))).rows.length,0);
  const user=await details(db,'users','user-00',new URL('https://admin.test/users/user-00'));
  assert.equal(user.record.providerId,'github');assert.ok(!JSON.stringify(user).includes('never-expose-this-token'));assert.equal(user.related[0].planId,'free');
 }finally{sql.close();}
});
test('overview and details preserve the platform subscription fallback',async()=>{
 const {sql,db}=fixture();try{
  let summary=await overview(db);assert.equal(summary.users,27);assert.equal(summary.plans.find(plan=>plan.id==='free')?.count,1);
  const project=await details(db,'projects','project',new URL('https://admin.test/projects/project'));
  assert.equal(project.record.effectivePlan,'free');assert.equal(project.record.rawStatus,'cancelled');assert.equal(project.record.usedBytes,0);assert.equal(project.related[0].role,'owner');
  sql.exec("UPDATE project_subscription SET status='active'");summary=await overview(db);assert.equal(summary.plans.find(plan=>plan.id==='team')?.count,1);
 }finally{sql.close();}
});
test('reject invalid pages and missing records',async()=>{
 const {sql,db}=fixture();try{
  for(const page of ['0','-1','1.5','4001','NaN'])await assert.rejects(listRecords(db,'users',new URL(`https://admin.test/users?page=${page}`)));
  await assert.rejects(details(db,'users','missing',new URL('https://admin.test/users/missing')));
 }finally{sql.close();}
});

test('catalog migration preserves existing subscriptions and seeds the current limits', async () => {
 const {sql,db}=fixture(sql=>sql.exec(`
  INSERT INTO user (id,name,email,createdAt,updatedAt) VALUES ('legacy-user','Legacy','legacy@example.test',0,0);
  INSERT INTO project (id,name,createdAt) VALUES ('legacy','Legacy',0);
  INSERT INTO project_subscription (projectId,planId,status,provider,periodStartAt,periodEndAt,createdAt,updatedAt)
  VALUES ('legacy','creator','simulated','local',0,NULL,1,2);
 `));
 try {
  const subscription=sql.prepare("SELECT planId,status,provider,periodStartAt,periodEndAt,updatedAt FROM project_subscription WHERE projectId='legacy'").get();
  assert.deepEqual({...subscription},{planId:'creator',status:'simulated',provider:'local',periodStartAt:0,periodEndAt:null,updatedAt:2});
  const catalog=await plans(db);
  assert.deepEqual(catalog.map(plan=>[plan.id,plan.maxFileSizeBytes,plan.storageBytes]),[
   ['free',2*1024**2,100*1024**2],['creator',8*1024**3,100*1024**3],['team',8*1024**3,1024**4]
  ]);
  assert.equal((await details(db,'projects','legacy',new URL('https://admin.test/projects/legacy'))).record.effectivePlan,'creator');
 } finally {sql.close();}
});
test('admin lists, overview and details agree on expired and scheduled subscriptions',async()=>{
 const {sql,db}=fixture();try{
  for(const condition of ['periodEndAt = 0',`periodStartAt = ${Date.now()+60000}, periodEndAt = NULL`]) {
   sql.exec(`UPDATE project_subscription SET status='active', ${condition}`);
   assert.equal((await overview(db)).plans.find(plan=>plan.id==='free')?.count,1);
   assert.equal((await listRecords(db,'projects',new URL('https://admin.test/projects'))).rows[0].planId,'free');
   assert.equal((await details(db,'projects','project',new URL('https://admin.test/projects/project'))).record.effectivePlan,'free');
  }
 }finally{sql.close();}
});
test('admin reads current catalog changes and rejects incomplete configuration',async()=>{
 const {sql,db}=fixture();try{
  sql.exec("UPDATE plan_catalog SET storageBytes=209715200, revision=revision+1 WHERE id='free'");
  assert.equal((await plans(db)).find(plan=>plan.id==='free')?.storageBytes,209715200);
  assert.equal((await details(db,'projects','project',new URL('https://admin.test/projects/project'))).record.storageBytes,209715200);
  sql.exec("DELETE FROM plan_catalog WHERE id='creator'");
  await assert.rejects(plans(db),cause=>typeof cause==='object' && cause!==null && 'status' in cause && cause.status===503);
 }finally{sql.close();}
});
