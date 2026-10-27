import { z } from 'zod';
import { nodeIdSchema, pageRequestSchema } from '@q2q/punch-discovery';
import { policyInputSchema, policyFingerprint, operationSchema, managedPunchSchema, type PolicyOutcome } from '@q2q/punch-management';
const columns=`n.node_id, n.service_type, n.region, n.version, n.advertise_url, n.public_key,
 n.identity_version, n.last_seen, n.expires_at, n.accepting_new_connections, n.metrics_json,
 COALESCE(p.enabled,1) AS enabled, COALESCE(p.revision,0) AS revision`;
function node(row:Record<string,unknown>,now:number){
 return managedPunchSchema.parse({...row,online:Number(row.expires_at)>now,
  accepting_new_connections:row.accepting_new_connections===1,enabled:row.enabled===1,
  metrics:row.metrics_json===null?null:JSON.parse(String(row.metrics_json))});
}
export async function listPunches(db:D1Database,input:unknown){
 const request=pageRequestSchema.parse(input);const now=Math.floor(Date.now()/1000);
 const limit=Math.min(request.limit,50);
 const {results}=await db.prepare(`SELECT ${columns} FROM nodes n LEFT JOIN punch_policy p ON p.node_id=n.node_id
  WHERE n.service_type='punch' AND n.node_id>? ORDER BY n.node_id LIMIT ?`).bind(request.after??'',limit+1).all<Record<string,unknown>>();
 const page=results.slice(0,limit).map(row=>node(row,now));
 return {server_time:now,nodes:page,next_cursor:results.length>limit?page.at(-1)!.node_id:null};
}
export async function getPunch(db:D1Database,input:unknown){
 const {nodeId}=z.strictObject({nodeId:nodeIdSchema}).parse(input);
 const row=await db.prepare(`SELECT ${columns} FROM nodes n LEFT JOIN punch_policy p ON p.node_id=n.node_id WHERE n.service_type='punch' AND n.node_id=?`).bind(nodeId).first<Record<string,unknown>>();
 return row?node(row,Math.floor(Date.now()/1000)):null;
}
export async function getOperation(db:D1Database,input:unknown){
 const {requestId}=z.strictObject({requestId:z.uuid()}).parse(input);
 const row=await db.prepare('SELECT * FROM punch_operation WHERE request_id=? AND applied_at IS NOT NULL').bind(requestId).first<Record<string,unknown>>();
 return row?operationSchema.parse({requestId:row.request_id,targetId:row.node_id,actorId:row.actor_id,actorEmail:row.actor_email,
  reason:row.reason,fingerprint:row.fingerprint,createdAt:row.created_at,
  before:{enabled:row.before_enabled===1,revision:row.before_revision},after:{enabled:row.after_enabled===1,revision:row.after_revision}}):null;
}
export async function setPolicy(db:D1Database,input:unknown):Promise<PolicyOutcome>{
 const parsed=policyInputSchema.safeParse(input);
 if(!parsed.success)return {ok:false,code:'invalid_request'};
 const value=parsed.data;const hash=await policyFingerprint(value);
 const existing=await getOperation(db,{requestId:value.requestId});
 if(existing)return existing.fingerprint===hash&&existing.actorId===value.actorId?{ok:true,operation:existing}:{ok:false,code:'request_conflict'};
 const current=await getPunch(db,{nodeId:value.nodeId});if(!current)return {ok:false,code:'not_found'};
 const now=Date.now();
 const pending=`EXISTS(SELECT 1 FROM punch_operation WHERE request_id=? AND fingerprint=? AND applied_at IS NULL)`;
 await db.batch([
  db.prepare(`INSERT INTO punch_operation(request_id,actor_id,actor_email,node_id,reason,fingerprint,before_enabled,before_revision,after_enabled,after_revision,created_at)
   SELECT ?,?,?,?,?,?,COALESCE(p.enabled,1),COALESCE(p.revision,0),?,?,? FROM nodes n LEFT JOIN punch_policy p ON p.node_id=n.node_id
   WHERE n.node_id=? AND n.service_type='punch' AND COALESCE(p.revision,0)=? ON CONFLICT(request_id) DO NOTHING`)
   .bind(value.requestId,value.actorId,value.actorEmail,value.nodeId,value.reason,hash,Number(value.enabled),value.revision+1,now,value.nodeId,value.revision),
  db.prepare(`INSERT INTO punch_policy(node_id,enabled,revision,updated_at) SELECT ?,?,?,? WHERE ${pending}
   ON CONFLICT(node_id) DO UPDATE SET enabled=excluded.enabled,revision=excluded.revision,updated_at=excluded.updated_at WHERE punch_policy.revision=?`)
   .bind(value.nodeId,Number(value.enabled),value.revision+1,now,value.requestId,hash,value.revision),
  db.prepare(`UPDATE punch_operation SET applied_at=? WHERE request_id=? AND fingerprint=? AND applied_at IS NULL
   AND EXISTS(SELECT 1 FROM punch_policy WHERE node_id=punch_operation.node_id AND revision=punch_operation.after_revision)`)
   .bind(now,value.requestId,hash)
 ]);
 const applied=await getOperation(db,{requestId:value.requestId});
 if(!applied)return {ok:false,code:'conflict'};
 return applied.fingerprint===hash&&applied.actorId===value.actorId?{ok:true,operation:applied}:{ok:false,code:'request_conflict'};
}
