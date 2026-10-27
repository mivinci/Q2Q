import assert from 'node:assert/strict';
import{test}from'node:test';
import{registerHooks}from'node:module';
import{policyFingerprint,type PolicyInput,type PunchManagementClient}from'@q2q/punch-management';
import{database}from'./database.ts';
registerHooks({resolve(specifier,context,next){try{return next(specifier,context);}catch(cause){if(context.parentURL?.startsWith(new URL('../src/',import.meta.url).href)&&specifier.startsWith('.')&&specifier.endsWith('.js'))return next(specifier.slice(0,-3)+'.ts',context);throw cause;}}});
const{updatePunchPolicy,reconcilePunchRequest,retryPunchRequest}=await import('../src/lib/server/registry-management.ts');
const{getAudit,listAudit}=await import('../src/lib/server/audit.ts');
const actor={id:'operator',email:'operator@example.test'};
function form(){const result=new FormData();for(const[key,value]of Object.entries({requestId:crypto.randomUUID(),revision:'0',enabled:'0',reason:'Drain this node'}))result.set(key,value);return result;}
function registry(){
 const operations=new Map<string,unknown>();let calls=0;let lose=false;let revision=0;
 const client={async setPolicy(input:unknown){calls++;const value=input as PolicyInput;if(operations.has(value.requestId))return{ok:true,operation:operations.get(value.requestId)};
  if(value.revision!==revision)return{ok:false,code:'conflict'};
  const operation={requestId:value.requestId,targetId:value.nodeId,actorId:value.actorId,actorEmail:value.actorEmail,reason:value.reason,fingerprint:await policyFingerprint(value),createdAt:Date.now(),before:{enabled:true,revision},after:{enabled:value.enabled,revision:++revision}};
  operations.set(value.requestId,operation);if(lose)throw Error('Response lost after commit');return{ok:true,operation};},
  async getOperation(input:unknown){return operations.get((input as{requestId:string}).requestId)??null;}
 }as unknown as PunchManagementClient;
 return{client,operations,get calls(){return calls;},lose(){lose=true;}};
}
test('RPC result is recorded once and duplicate requests return the original result',async()=>{
 const{sql,db}=database();const rpc=registry();try{
  const input=form();const result=await updatePunchPolicy(db,rpc.client,actor,'punch',input);
  assert.equal((await getAudit(db,result.requestId)).status,'applied');
  await updatePunchPolicy(db,rpc.client,actor,'punch',input);assert.equal(rpc.calls,1);
  input.set('enabled','1');await assert.rejects(updatePunchPolicy(db,rpc.client,actor,'punch',input));
  assert.equal((await listAudit(db,new URL('https://admin.test/audit?action=punch.policy'))).rows.length,1);
 }finally{sql.close();}
});
test('lost response stays pending and GET reconciliation confirms the Registry audit',async()=>{
 const{sql,db}=database();const rpc=registry();try{
  rpc.lose();const input=form();await assert.rejects(updatePunchPolicy(db,rpc.client,actor,'punch',input));
  const id=String(input.get('requestId'));assert.equal((await getAudit(db,id)).status,'pending');
  await reconcilePunchRequest(db,rpc.client,id);assert.equal((await getAudit(db,id)).status,'applied');assert.equal(rpc.calls,1);
 }finally{sql.close();}
});
test('original retry after an uncertain result is safe and only the initiating actor can retry',async()=>{
 const{sql,db}=database();const rpc=registry();try{
  rpc.lose();const input=form();await assert.rejects(updatePunchPolicy(db,rpc.client,actor,'punch',input));
  const id=String(input.get('requestId'));
  await assert.rejects(retryPunchRequest(db,rpc.client,{id:'other',email:'other@example.test'},id));
  await retryPunchRequest(db,rpc.client,actor,id);assert.equal((await getAudit(db,id)).status,'applied');assert.equal(rpc.operations.size,1);
 }finally{sql.close();}
});
test('unreachable Registry does not become a false rejected or applied outcome',async()=>{
 const{sql,db}=database();try{
  const client={async setPolicy(){throw Error('unavailable');},async getOperation(){throw Error('unavailable');}}as unknown as PunchManagementClient;
  const input=form();await assert.rejects(updatePunchPolicy(db,client,actor,'punch',input));const id=String(input.get('requestId'));
  await reconcilePunchRequest(db,client,id);assert.equal((await getAudit(db,id)).status,'pending');
 }finally{sql.close();}
});
test('a stale remote policy is recorded as rejected without silently retrying a new revision',async()=>{
 const{sql,db}=database();const rpc=registry();try{
  await updatePunchPolicy(db,rpc.client,actor,'punch',form());
  const input=form();await assert.rejects(updatePunchPolicy(db,rpc.client,actor,'punch',input));
  assert.equal((await getAudit(db,String(input.get('requestId')))).status,'rejected');assert.equal(rpc.operations.size,1);
 }finally{sql.close();}
});
test('mismatched RPC responses cannot be recorded as applied',async()=>{
 const{sql,db}=database();try{
  const client={async setPolicy(raw:unknown){const input=raw as PolicyInput;return{ok:true,operation:{requestId:input.requestId,targetId:input.nodeId,actorId:input.actorId,actorEmail:input.actorEmail,reason:input.reason,fingerprint:'0'.repeat(64),createdAt:Date.now(),before:{enabled:true,revision:0},after:{enabled:false,revision:1}}};}}as unknown as PunchManagementClient;
  const input=form();await assert.rejects(updatePunchPolicy(db,client,actor,'punch',input));
  const audit=await getAudit(db,String(input.get('requestId')));assert.equal(audit.status,'pending');assert.equal(audit.requestedEnabled,0);
 }finally{sql.close();}
});
