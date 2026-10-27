import assert from 'node:assert/strict';
import { test } from 'node:test';
import { database as fixture } from './database.ts';
import { updatePlan, setSubscription, MutationError } from '../src/lib/server/mutations.ts';
import { listAudit, getAudit } from '../src/lib/server/audit.ts';

const actor = { id: 'https://test.cloudflareaccess.com#operator', email: 'operator@example.test' };

function plan(values: Record<string,string> = {}) {
  const form = new FormData();
  for (const [key,value] of Object.entries({requestId:crypto.randomUUID(),revision:'1',reason:'Adjust capacity for this project',maxFileSizeBytes:'2097152',storageBytes:'209715200',platformStorage:'on',platformNetwork:'on',customTrack:'on',...values})) form.set(key,value);
  return form;
}
function subscription(values: Record<string,string> = {}) {
  const form = new FormData();
  for (const [key,value] of Object.entries({requestId:crypto.randomUUID(),revision:'0',reason:'Grant trial access',planId:'creator',planRevisions:'{"free":1,"creator":1,"team":1}',startAt:'',endAt:'',...values})) form.set(key,value);
  return form;
}
const conflict = (cause: unknown) => cause instanceof MutationError && cause.status === 409;

test('plan edit and before/after audit commit once under retry', async () => {
  const {sql,db}=fixture(); try {
    const form=plan(); const result=await updatePlan(db,actor,'free',form);
    assert.equal(result.replayed,false);
    const row=sql.prepare("SELECT storageBytes,revision FROM plan_catalog WHERE id='free'").get()!;
    assert.equal(row.storageBytes,209715200); assert.equal(row.revision,2);
    const record=await getAudit(db,result.requestId);
    assert.equal(record.actorId,actor.id); assert.equal(record.reason,'Adjust capacity for this project');
    assert.equal(JSON.parse(record.beforeJson).storageBytes,104857600);
    assert.equal(JSON.parse(record.afterJson).storageBytes,209715200);
    assert.equal(JSON.parse(record.afterJson).affectedProjects,1);
    assert.equal((await updatePlan(db,actor,'free',form)).replayed,true);
    assert.equal(sql.prepare('SELECT COUNT(*) AS count FROM admin_operation').get()!.count,1);
    assert.equal(sql.prepare("SELECT revision FROM plan_catalog WHERE id='free'").get()!.revision,2);
    form.set('reason','Different action');
    await assert.rejects(updatePlan(db,actor,'free',form),conflict);
  } finally {sql.close();}
});
test('reused request IDs require the original actor',async()=>{
  const {sql,db}=fixture();try{
    const form=plan();await updatePlan(db,actor,'free',form);
    await assert.rejects(updatePlan(db,{id:'other',email:'other@example.test'},'free',form),conflict);
  }finally{sql.close();}
});
test('concurrent identical retries apply once; competing edits return conflict',async()=>{
  const {sql,db}=fixture();try{
    const form=plan();const results=await Promise.all([updatePlan(db,actor,'free',form),updatePlan(db,actor,'free',form)]);
    assert.equal(results.filter(value=>!value.replayed).length,1);
    const edits=await Promise.allSettled([updatePlan(db,actor,'free',plan({revision:'2'})),updatePlan(db,actor,'free',plan({revision:'2',storageBytes:'314572800'}))]);
    assert.equal(edits.filter(value=>value.status==='fulfilled').length,1);
    assert.equal(edits.filter(value=>value.status==='rejected'&&conflict(value.reason)).length,1);
    assert.equal(sql.prepare('SELECT COUNT(*) AS count FROM admin_operation').get()!.count,2);
  }finally{sql.close();}
});
test('business write failure rolls back the audit and remains safely retryable',async()=>{
  const {sql,db}=fixture();try{
    sql.exec("CREATE TRIGGER fail_plan BEFORE UPDATE ON plan_catalog BEGIN SELECT RAISE(ABORT,'test failure'); END;");
    const form=plan();await assert.rejects(updatePlan(db,actor,'free',form));
    assert.equal(sql.prepare('SELECT COUNT(*) AS count FROM admin_operation').get()!.count,0);
    assert.equal(sql.prepare("SELECT revision FROM plan_catalog WHERE id='free'").get()!.revision,1);
    sql.exec('DROP TRIGGER fail_plan');await updatePlan(db,actor,'free',form);
  }finally{sql.close();}
});
test('transport loss after commit is resolved by retrying the same request',async()=>{
  const {sql,db}=fixture();try{
    const flaky={...db,batch:async (...args:Parameters<D1Database['batch']>)=>{await db.batch(...args);throw Error('response lost');}} as D1Database;
    const form=plan();await assert.rejects(updatePlan(flaky,actor,'free',form));
    const replay=await updatePlan(db,actor,'free',form);assert.equal(replay.replayed,true);
    assert.equal(sql.prepare("SELECT revision FROM plan_catalog WHERE id='free'").get()!.revision,2);
  }finally{sql.close();}
});
test('manual subscriptions record their source, interval and selected plan snapshot',async()=>{
  const {sql,db}=fixture();try{
    const form=subscription({startAt:'2027-01-01T00:00:00.000',endAt:'2027-02-01T00:00:00.000'});
    const result=await setSubscription(db,actor,'project',form);
    const row=sql.prepare("SELECT * FROM project_subscription WHERE projectId='project'").get()!;
    assert.equal(row.provider,'manual');assert.equal(row.status,'active');assert.equal(row.revision,1);
    assert.equal(row.periodStartAt,Date.parse('2027-01-01T00:00:00Z'));
    const audit=await getAudit(db,result.requestId);
    assert.equal(JSON.parse(audit.beforeJson),null);
    assert.equal(JSON.parse(audit.afterJson).planSnapshot.storageBytes,100*1024**3);
    assert.equal((await setSubscription(db,actor,'project',form)).replayed,true);
    sql.exec("UPDATE project_subscription SET planId='team'");
    assert.equal(sql.prepare("SELECT revision FROM project_subscription WHERE projectId='project'").get()!.revision,2);
    await assert.rejects(setSubscription(db,actor,'project',subscription({revision:'1'})),conflict);
  }finally{sql.close();}
});
test('a changed plan definition invalidates the subscription preview',async()=>{
  const {sql,db}=fixture();try{
    sql.exec("UPDATE plan_catalog SET revision=revision+1 WHERE id='creator'");
    await assert.rejects(setSubscription(db,actor,'project',subscription()),conflict);
    assert.equal(sql.prepare('SELECT COUNT(*) AS count FROM project_subscription').get()!.count,0);
    assert.equal(sql.prepare('SELECT COUNT(*) AS count FROM admin_operation').get()!.count,0);
  }finally{sql.close();}
});
test('reject unsafe limits, unknown fields, duplicate fields, blank reasons and invalid periods',async()=>{
  const {sql,db}=fixture();try{
    for(const values of [{storageBytes:'-1'},{maxFileSizeBytes:'8589934593'},{maxFileSizeBytes:'1.2'},{reason:' '},{unknownFeature:'on'}]) await assert.rejects(updatePlan(db,actor,'free',plan(values)),cause=>cause instanceof MutationError&&cause.status===400);
    const duplicate=plan();duplicate.append('revision','1');await assert.rejects(updatePlan(db,actor,'free',duplicate));
    for(const values of [{startAt:'2027-02-30T00:00'},{startAt:'2027-02-01T00:00',endAt:'2027-01-01T00:00'},{planId:'unknown'}]) await assert.rejects(setSubscription(db,actor,'project',subscription(values)));
    assert.equal(sql.prepare('SELECT COUNT(*) AS count FROM admin_operation').get()!.count,0);
  }finally{sql.close();}
});
test('audit search filters committed records and never exposes request fingerprints',async()=>{
  const {sql,db}=fixture();try{
    await updatePlan(db,actor,'free',plan());
    await setSubscription(db,actor,'project',subscription({planRevisions:'{"free":2,"creator":1,"team":1}'}));
    const records=await listAudit(db,new URL('https://admin.test/audit?action=subscription.set&q=project'));
    assert.equal(records.rows.length,1);assert.equal(records.rows[0].action,'subscription.set');
    assert.ok(!('fingerprint' in records.rows[0]));
    await assert.rejects(listAudit(db,new URL('https://admin.test/audit?from=2027-02-30')));
  }finally{sql.close();}
});

test('existing subscriptions update once and preserve creation time',async()=>{
  const {sql,db}=fixture();try{
    await setSubscription(db,actor,'project',subscription());
    const original=sql.prepare("SELECT createdAt FROM project_subscription WHERE projectId='project'").get()!;
    const update=subscription({revision:'1',planId:'team'});
    const result=await setSubscription(db,actor,'project',update);
    const row=sql.prepare("SELECT planId,revision,createdAt FROM project_subscription WHERE projectId='project'").get()!;
    assert.equal(row.planId,'team');assert.equal(row.revision,2);assert.equal(row.createdAt,original.createdAt);
    const audit=await getAudit(db,result.requestId);
    assert.equal(JSON.parse(audit.beforeJson).planId,'creator');assert.equal(JSON.parse(audit.afterJson).planId,'team');
    assert.equal((await setSubscription(db,actor,'project',update)).replayed,true);
  }finally{sql.close();}
});
test('concurrent first grants cannot overwrite each other',async()=>{
  const {sql,db}=fixture();try{
    const results=await Promise.allSettled([setSubscription(db,actor,'project',subscription()),setSubscription(db,actor,'project',subscription({planId:'team'}))]);
    assert.equal(results.filter(value=>value.status==='fulfilled').length,1);
    assert.equal(results.filter(value=>value.status==='rejected'&&conflict(value.reason)).length,1);
    assert.equal(sql.prepare('SELECT COUNT(*) AS count FROM admin_operation').get()!.count,1);
  }finally{sql.close();}
});
test('subscription failures roll back history and the revision',async()=>{
  const {sql,db}=fixture();try{
    await setSubscription(db,actor,'project',subscription());
    sql.exec("CREATE TRIGGER fail_subscription BEFORE UPDATE ON project_subscription BEGIN SELECT RAISE(ABORT,'test failure'); END;");
    await assert.rejects(setSubscription(db,actor,'project',subscription({revision:'1',planId:'team'})));
    assert.equal(sql.prepare('SELECT COUNT(*) AS count FROM admin_operation').get()!.count,1);
    assert.equal(sql.prepare("SELECT planId FROM project_subscription WHERE projectId='project'").get()!.planId,'creator');
  }finally{sql.close();}
});
test('the Free fallback definition participates in subscription preview conflicts',async()=>{
  const {sql,db}=fixture();try{
    sql.exec("UPDATE plan_catalog SET revision=revision+1 WHERE id='free'");
    await assert.rejects(setSubscription(db,actor,'project',subscription()),conflict);
    assert.equal(sql.prepare('SELECT COUNT(*) AS count FROM admin_operation').get()!.count,0);
  }finally{sql.close();}
});
test('audit pagination is stable and UTC date filters include the full day',async()=>{
  const {sql,db}=fixture();try{
    for(let revision=1;revision<=26;revision++) await updatePlan(db,actor,'free',plan({revision:String(revision)}));
    const day=new Date().toISOString().slice(0,10);
    const first=await listAudit(db,new URL(`https://admin.test/audit?from=${day}&to=${day}`));
    const second=await listAudit(db,new URL('https://admin.test/audit?page=2'));
    assert.equal(first.rows.length,25);assert.equal(first.hasNext,true);assert.equal(second.rows.length,1);
    assert.ok(!first.rows.some(row=>row.requestId===second.rows[0].requestId));
    assert.equal((await listAudit(db,new URL('https://admin.test/audit?to=1970-01-01'))).rows.length,0);
  }finally{sql.close();}
});

test('definition updates outside the editor invalidate its original revision',async()=>{
  const {sql,db}=fixture();try{
    sql.exec("UPDATE plan_catalog SET storageBytes=314572800 WHERE id='free'");
    assert.equal(sql.prepare("SELECT revision FROM plan_catalog WHERE id='free'").get()!.revision,2);
    await assert.rejects(updatePlan(db,actor,'free',plan()),conflict);
    assert.equal(sql.prepare('SELECT COUNT(*) AS count FROM admin_operation').get()!.count,0);
  }finally{sql.close();}
});
