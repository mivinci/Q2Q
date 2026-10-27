import assert from 'node:assert/strict';
import { test } from 'node:test';
import { registerHooks } from 'node:module';
import { database } from './database.ts';
registerHooks({ resolve(specifier, context, next) {
  try { return next(specifier, context); }
  catch (cause) {
    if (context.parentURL?.startsWith(new URL('../src/', import.meta.url).href) && specifier.startsWith('.') && specifier.endsWith('.js')) return next(specifier.slice(0,-3)+'.ts', context);
    throw cause;
  }
} });
const { manageUser } = await import('../src/lib/server/user-mutations.ts');
const { getAudit, listAudit } = await import('../src/lib/server/audit.ts');
const { prepareAccountSession, authorizeAccountSession } = await import('@q2q/accounts');
const actor={id:'access-operator',email:'operator@example.test'};
function form(mode:string, revision=1, requestId=crypto.randomUUID()) {
  const value=new FormData();
  for(const [key,item] of Object.entries({requestId,revision:String(revision),reason:'Review account access',mode})) value.set(key,item);
  return value;
}
function createSession(sql: ReturnType<typeof database>['sql'], id:string, user='user', version=1) {
  sql.prepare('INSERT INTO session (id,userId,token,expiresAt,createdAt,updatedAt,authVersion) VALUES (?,?,?,?,0,0,?)')
    .run(id,user,`test-only-${id}`,Date.now()+60000,version);
}
const denied=(cause:unknown)=>cause instanceof Error && 'status' in cause && cause.status===409;

test('migration preserves existing accounts, sessions and operation history',async()=>{
  const {sql,db}=database((name,sql)=>{
    if(name!=='0019_user_access.sql') return;
    sql.exec(`INSERT INTO user (id,name,email,createdAt,updatedAt) VALUES ('legacy','Legacy','legacy@example.test',0,0);
      INSERT INTO session (id,userId,token,expiresAt,createdAt,updatedAt) VALUES ('legacy-session','legacy','test-only',9999999999999,0,0);
      INSERT INTO admin_operation (requestId,actorId,actorEmail,action,targetId,reason,fingerprint,beforeJson,afterJson,revision,createdAt,appliedAt)
      VALUES ('00000000-0000-0000-0000-000000000001','operator','operator@example.test','plan.update','free','Previous operation','aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','{}','{}',1,1,1);`);
  });
  try{
    assert.equal(sql.prepare("SELECT authVersion FROM session WHERE id='legacy-session'").get()!.authVersion,1);
    assert.equal((await authorizeAccountSession(db,'legacy-session','legacy'))?.id,'legacy');
    assert.equal((await getAudit(db,'00000000-0000-0000-0000-000000000001')).reason,'Previous operation');
  }finally{sql.close();}
});
test('disable revokes all sessions atomically while preserving projects and other members',async()=>{
  const {sql,db}=database();try{
    sql.exec(`INSERT INTO user (id,name,email,createdAt,updatedAt) VALUES ('other','Other','other@example.test',0,0);
      INSERT INTO project_member (projectId,userId,role,createdAt) VALUES ('project','user','owner',0),('project','other','member',0);
      INSERT INTO file_record (id,projectId,createdBy,name,size,status,createdAt,updatedAt) VALUES ('file','project','user','Example',1,'pending',0,0);`);
    createSession(sql,'own');createSession(sql,'other-session','other');
    assert.equal((await authorizeAccountSession(db,'own','user'))?.id,'user');
    const request=form('disable');const result=await manageUser(db,actor,'user',request);
    const account=sql.prepare("SELECT disabledAt,disabledReason,accessRevision,authVersion FROM user WHERE id='user'").get()!;
    assert.equal(typeof account.disabledAt,'number');assert.equal(account.disabledReason,'Review account access');
    assert.equal(account.accessRevision,2);assert.equal(account.authVersion,2);
    assert.equal(await authorizeAccountSession(db,'own','user'),null);
    assert.equal((await authorizeAccountSession(db,'other-session','other'))?.id,'other');
    assert.equal(sql.prepare("SELECT COUNT(*) AS count FROM project_member WHERE projectId='project'").get()!.count,2);
    assert.equal(sql.prepare('SELECT COUNT(*) AS count FROM project').get()!.count,1);
    assert.equal(sql.prepare('SELECT COUNT(*) AS count FROM file_record').get()!.count,1);
    const audit=await getAudit(db,result.requestId);
    assert.equal(audit.action,'user.disable');assert.equal(JSON.parse(audit.afterJson).storedSessions,0);
    assert.ok(!JSON.stringify(audit).includes('test-only-own'));
    assert.equal((await manageUser(db,actor,'user',request)).replayed,true);
    await assert.rejects(prepareAccountSession(db,{userId:'user'}));
    assert.throws(()=>createSession(sql,'blocked','user',2));
  }finally{sql.close();}
});
test('restoring requires a fresh session and never revives the old generation',async()=>{
  const {sql,db}=database();try{
    createSession(sql,'old');await manageUser(db,actor,'user',form('disable'));
    await manageUser(db,actor,'user',form('restore',2));
    assert.equal(sql.prepare("SELECT disabledAt FROM user WHERE id='user'").get()!.disabledAt,null);
    assert.equal(await authorizeAccountSession(db,'old','user'),null);
    assert.throws(()=>createSession(sql,'stale','user',1));
    const fresh=await prepareAccountSession(db,{userId:'user',authVersion:999});
    assert.equal(fresh.data.authVersion,3);
    createSession(sql,'fresh','user',fresh.data.authVersion);
    assert.equal((await authorizeAccountSession(db,'fresh','user'))?.id,'user');
  }finally{sql.close();}
});
test('revoke keeps login enabled and rejects a session prepared before the operation',async()=>{
  const {sql,db}=database();try{
    createSession(sql,'old');const prepared=await prepareAccountSession(db,{userId:'user'});
    await manageUser(db,actor,'user',form('revoke'));
    assert.equal(sql.prepare("SELECT disabledAt FROM user WHERE id='user'").get()!.disabledAt,null);
    assert.equal(await authorizeAccountSession(db,'old','user'),null);
    assert.throws(()=>createSession(sql,'in-flight','user',prepared.data.authVersion));
    const fresh=await prepareAccountSession(db,{userId:'user'});
    createSession(sql,'new','user',fresh.data.authVersion);
    assert.equal((await authorizeAccountSession(db,'new','user'))?.id,'user');
  }finally{sql.close();}
});
test('disable versus session creation is safe in either database ordering',async()=>{
  const {sql,db}=database();try{
    const prepared=await prepareAccountSession(db,{userId:'user'});
    createSession(sql,'before','user',prepared.data.authVersion);
    await manageUser(db,actor,'user',form('disable'));
    assert.equal(sql.prepare("SELECT COUNT(*) AS count FROM session WHERE userId='user'").get()!.count,0);
    assert.throws(()=>createSession(sql,'after','user',prepared.data.authVersion));
  }finally{sql.close();}
});
test('concurrent and stale management actions cannot silently overwrite each other',async()=>{
  const {sql,db}=database();try{
    const results=await Promise.allSettled([manageUser(db,actor,'user',form('disable')),manageUser(db,actor,'user',form('revoke'))]);
    assert.equal(results.filter(value=>value.status==='fulfilled').length,1);
    assert.equal(results.filter(value=>value.status==='rejected'&&denied(value.reason)).length,1);
    assert.equal(sql.prepare('SELECT COUNT(*) AS count FROM admin_operation').get()!.count,1);
  }finally{sql.close();}
});
test('session deletion failure rolls back account state and audit',async()=>{
  const {sql,db}=database();try{
    createSession(sql,'keep');sql.exec("CREATE TRIGGER fail_delete BEFORE DELETE ON session BEGIN SELECT RAISE(ABORT,'test failure'); END;");
    await assert.rejects(manageUser(db,actor,'user',form('disable')));
    const account=sql.prepare("SELECT disabledAt,authVersion FROM user WHERE id='user'").get()!;
    assert.equal(account.disabledAt,null);assert.equal(account.authVersion,1);
    assert.equal(sql.prepare('SELECT COUNT(*) AS count FROM admin_operation').get()!.count,0);
    assert.equal((await authorizeAccountSession(db,'keep','user'))?.id,'user');
  }finally{sql.close();}
});
test('persisted account and expiry checks reject stale library session objects',async()=>{
  const {sql,db}=database();try{
    createSession(sql,'cookie');
    assert.equal(await authorizeAccountSession(db,'cookie','other-user'),null);
    assert.equal(await authorizeAccountSession(db,'cookie','user',Date.now()+60001),null);
    sql.exec("UPDATE user SET authVersion=2 WHERE id='user'");
    assert.equal(await authorizeAccountSession(db,'cookie','user'),null);
    assert.throws(()=>sql.exec("UPDATE session SET expiresAt=9999999999999 WHERE id='cookie'"));
  }finally{sql.close();}
});
test('user audit filters and request IDs retain the same replay protections',async()=>{
  const {sql,db}=database();try{
    const request=form('revoke');await manageUser(db,actor,'user',request);
    assert.equal((await listAudit(db,new URL('https://admin.test/audit?action=user.sessions.revoke'))).rows.length,1);
    await assert.rejects(manageUser(db,{id:'other-operator',email:'other@example.test'},'user',request),denied);
    request.set('mode','disable');await assert.rejects(manageUser(db,actor,'user',request),denied);
    assert.equal(sql.prepare("SELECT disabledAt FROM user WHERE id='user'").get()!.disabledAt,null);
  }finally{sql.close();}
});

test('ISO session expiry and millisecond boundaries match legacy numeric rows',async()=>{
  const {sql,db}=database();try{
    createSession(sql,'iso');
    for(const milliseconds of [0,1,123,999]) {
      const expiry=Date.UTC(2027,0,1)+milliseconds;
      sql.prepare("UPDATE session SET expiresAt=? WHERE id='iso'").run(new Date(expiry).toISOString());
      assert.equal((await authorizeAccountSession(db,'iso','user',expiry-1))?.id,'user');
      assert.equal(await authorizeAccountSession(db,'iso','user',expiry),null);
    }
    const {details}=await import('../src/lib/server/queries.ts');
    assert.equal((await details(db,'users','user',new URL('https://admin.test/users/user'))).record.activeSessions,1);
    sql.prepare("UPDATE session SET expiresAt=? WHERE id='iso'").run(new Date(Date.now()-1000).toISOString());
    assert.equal((await details(db,'users','user',new URL('https://admin.test/users/user'))).record.activeSessions,0);
    sql.prepare("UPDATE session SET expiresAt='invalid-time' WHERE id='iso'").run();
    assert.equal(await authorizeAccountSession(db,'iso','user'),null);
  }finally{sql.close();}
});
test('admin dates display Better Auth ISO timestamps and legacy milliseconds consistently',async()=>{
  const {date}=await import('../src/lib/format.ts');
  assert.equal(date(0,'en'),date('1970-01-01T00:00:00.000Z','en'));
  assert.equal(date(0,'en'),date('1970-01-01 00:00:00','en'));
  assert.equal(date('invalid-time','en'),'—');
});
