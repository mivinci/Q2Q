import { z } from 'zod';
import { nodeIdSchema, integer, metricsSchema, punchUrlSchema, punchPublicKeySchema } from '@q2q/punch-discovery';
export const policyInputSchema = z.strictObject({
  requestId: z.uuid(), nodeId: nodeIdSchema, enabled: z.boolean(),
  revision: integer.max(Number.MAX_SAFE_INTEGER - 1), reason: z.string().trim().min(1).max(1000),
  actorId: z.string().min(1).max(1024), actorEmail: z.string().email().max(320)
});
export type PolicyInput = z.infer<typeof policyInputSchema>;
export const operationSchema = z.object({
  requestId: z.uuid(), targetId: nodeIdSchema, actorId: z.string(), actorEmail: z.string(), reason: z.string(),
  fingerprint: z.string().regex(/^[a-f0-9]{64}$/), createdAt: integer,
  before: z.object({enabled:z.boolean(),revision:integer}),
  after: z.object({enabled:z.boolean(),revision:integer.positive()})
}).refine(value=>value.after.revision===value.before.revision+1);
export const outcomeSchema = z.discriminatedUnion('ok',[
  z.object({ok:z.literal(true),operation:operationSchema}),
  z.object({ok:z.literal(false),code:z.enum(['invalid_request','not_found','conflict','request_conflict'])})
]);
export type PolicyOutcome = z.infer<typeof outcomeSchema>;
export const managedPunchSchema = z.object({
  node_id:nodeIdSchema,service_type:z.literal('punch'),region:z.string().nullable(),version:z.string(),
  advertise_url:punchUrlSchema,public_key:punchPublicKeySchema,identity_version:integer.positive(),
  online:z.boolean(),accepting_new_connections:z.boolean(),metrics:metricsSchema.nullable(),
  last_seen:integer,expires_at:integer,enabled:z.boolean(),revision:integer
});
export const managedPageSchema=z.object({server_time:integer,nodes:z.array(managedPunchSchema).max(50),next_cursor:nodeIdSchema.nullable()});
export interface PunchManagementClient {
  listPunches(input:unknown):Promise<unknown>;
  getPunch(input:unknown):Promise<unknown>;
  setPolicy(input:unknown):Promise<unknown>;
  getOperation(input:unknown):Promise<unknown>;
}
export async function policyFingerprint(input:PolicyInput):Promise<string>{
  const payload={requestId:input.requestId,nodeId:input.nodeId,enabled:input.enabled,revision:input.revision,reason:input.reason,actorId:input.actorId};
  const digest=await crypto.subtle.digest('SHA-256',new TextEncoder().encode(JSON.stringify(payload)));
  return Array.from(new Uint8Array(digest),value=>value.toString(16).padStart(2,'0')).join('');
}
