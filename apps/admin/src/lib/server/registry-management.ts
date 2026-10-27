import { policyInputSchema, policyFingerprint, outcomeSchema, operationSchema, managedPageSchema, managedPunchSchema, type PolicyInput, type PolicyOutcome, type PunchManagementClient } from '@q2q/punch-management';
import { parseOperation, MutationError, type Actor, type SavedOperation } from './mutations.js';
export const punchFields=['requestId','revision','reason','enabled'];
export function managementClient(binding:unknown){return binding as PunchManagementClient;}
async function within<T>(promise:Promise<T>,milliseconds=5000):Promise<T>{
 let timer:ReturnType<typeof setTimeout>;
 const timeout=new Promise<never>((_,reject)=>{timer=setTimeout(()=>reject(Error('management_unavailable')),milliseconds);});
 try{return await Promise.race([promise,timeout]);}finally{clearTimeout(timer!);}
}
interface Intent {requestId:string;actorId:string;actorEmail:string;targetId:string;fingerprint:string;payloadJson:string;status:string;resultJson:string|null}
function match(intent:Intent,input:PolicyInput,hash:string){
 if(intent.actorId!==input.actorId||intent.fingerprint!==hash)throw new MutationError('requestConflict',409);
}
function validateOutcome(intent:Intent,value:unknown):PolicyOutcome{
 const result=outcomeSchema.parse(value);
 if(result.ok&&(result.operation.requestId!==intent.requestId||result.operation.actorId!==intent.actorId||result.operation.targetId!==intent.targetId||result.operation.fingerprint!==intent.fingerprint))throw Error('management_unavailable');
 return result;
}
async function store(db:D1Database,intent:Intent,value:unknown){
 const result=validateOutcome(intent,value);
 await db.prepare(`UPDATE registry_request SET status=?,resultJson=?,updatedAt=? WHERE requestId=? AND fingerprint=? AND status='pending'`)
  .bind(result.ok?'applied':'rejected',JSON.stringify(result),Date.now(),intent.requestId,intent.fingerprint).run();
 return result;
}
function saved(result:PolicyOutcome,replayed:boolean):SavedOperation{
 if(!result.ok)throw new MutationError(result.code==='not_found'?'recordMissing':result.code==='request_conflict'?'requestConflict':result.code==='conflict'?'changeConflict':'invalidChanges',result.code==='not_found'?404:result.code==='invalid_request'?400:409);
 return {requestId:result.operation.requestId,revision:result.operation.after.revision,createdAt:result.operation.createdAt,replayed};
}
export async function updatePunchPolicy(db:D1Database,client:PunchManagementClient,actor:Actor,nodeId:string,form:FormData){
 const base=parseOperation(form,punchFields);const flags=form.getAll('enabled');
 if(flags.length!==1||!['1','0'].includes(String(flags[0])))throw new MutationError('invalidChanges');
 const parsed=policyInputSchema.safeParse({requestId:base.requestId,nodeId,enabled:flags[0]==='1',revision:base.revision,reason:base.reason,actorId:actor.id,actorEmail:actor.email});
 if(!parsed.success)throw new MutationError('invalidChanges');
 const input=parsed.data;const hash=await policyFingerprint(input);
 let intent=await db.prepare('SELECT * FROM registry_request WHERE requestId=?').bind(input.requestId).first<Intent>();
 const replayed=!!intent;
 if(!intent){
  if(await db.prepare('SELECT requestId FROM admin_operation WHERE requestId=?').bind(input.requestId).first())throw new MutationError('requestConflict',409);
  const now=Date.now();
  try{await db.prepare(`INSERT INTO registry_request(requestId,actorId,actorEmail,targetId,reason,fingerprint,payloadJson,status,createdAt,updatedAt)
   VALUES(?,?,?,?,?,?,?,'pending',?,?) ON CONFLICT(requestId) DO NOTHING`).bind(input.requestId,actor.id,actor.email,nodeId,input.reason,hash,JSON.stringify(input),now,now).run();}
  catch(cause){if(cause instanceof Error&&cause.message.includes('request_conflict'))throw new MutationError('requestConflict',409);throw cause;}
  intent=await db.prepare('SELECT * FROM registry_request WHERE requestId=?').bind(input.requestId).first<Intent>();
 }
 if(!intent)throw Error('management_unavailable');
 match(intent,input,hash);
 if(intent.resultJson)return saved(validateOutcome(intent,JSON.parse(intent.resultJson)),true);
 const payload=policyInputSchema.parse(JSON.parse(intent.payloadJson));
 return saved(await store(db,intent,await within(client.setPolicy(payload))),replayed);
}
export async function reconcilePunchRequest(db:D1Database,client:PunchManagementClient,requestId:string){
 const intent=await db.prepare("SELECT * FROM registry_request WHERE requestId=? AND status='pending'").bind(requestId).first<Intent>();
 if(!intent)return;
 try{
  const value=await within(client.getOperation({requestId}));
  if(value===null)return;
  const operation=operationSchema.parse(value);
  await store(db,intent,{ok:true,operation});
 }catch{/* Keep the durable intent pending; unavailable does not mean rejected. */}
}
export async function readManagedPunches(client:PunchManagementClient,after:string|null){
 return managedPageSchema.parse(await within(client.listPunches({limit:25,...(after?{after}:{})})));
}
export async function readManagedPunch(client:PunchManagementClient,nodeId:string){
 const value=await within(client.getPunch({nodeId}));
 return value===null?null:managedPunchSchema.parse(value);
}
export async function retryPunchRequest(db:D1Database,client:PunchManagementClient,actor:Actor,requestId:string){
 const intent=await db.prepare('SELECT * FROM registry_request WHERE requestId=?').bind(requestId).first<Intent>();
 if(!intent)throw new MutationError('recordMissing',404);
 if(intent.actorId!==actor.id)throw new MutationError('requestConflict',409);
 const payload=policyInputSchema.parse(JSON.parse(intent.payloadJson));
 const form=new FormData();
 for(const [key,value] of Object.entries({requestId:payload.requestId,revision:String(payload.revision),reason:payload.reason,enabled:payload.enabled?'1':'0'}))form.set(key,value);
 return updatePunchPolicy(db,client,actor,payload.nodeId,form);
}
