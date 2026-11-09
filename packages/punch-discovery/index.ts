import { z } from 'zod';
import { punchEndpointsSchema } from './endpoints.ts';
import type { PunchScheme } from './endpoints.ts';
export { canonicalPunchEndpoint, MAX_PUNCH_ENDPOINTS, MAX_PUNCH_ENDPOINT_LENGTH, punchEndpointSchema,
  punchEndpointsSchema, punchSchemeSchema, punchSchemesSchema } from './endpoints.ts';
export type { PunchScheme } from './endpoints.ts';
export const MAX_SERVICE_TOKEN_LENGTH = 32768;

export const integer = z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER);
export const nodeIdSchema = z.string().regex(/^[A-Za-z0-9_.-]{1,128}$/);
export const environmentSchema = z.string().regex(/^[a-z][a-z0-9-]{0,31}$/);
export const punchPublicKeySchema = z.string().regex(/^[A-Za-z0-9+/]{43}=$/).refine(value => {
  try {return atob(value).length === 32 && btoa(atob(value)) === value;} catch {return false;}
});
export const punchIdentitySchema = z.strictObject({
  endpoints: punchEndpointsSchema, public_key: punchPublicKeySchema, identity_version: integer.positive(),
});
export const punchAssignmentSchema = z.strictObject({
  punch_id: nodeIdSchema, endpoints: punchEndpointsSchema, public_key: punchPublicKeySchema,
});
export type PunchAssignment = z.infer<typeof punchAssignmentSchema>;
export const metricsSchema = z.strictObject({
  quic_connections: integer.nullable(), quic_connection_capacity: integer.nullable(),
  websocket_connections: integer.nullable(), websocket_connection_capacity: integer.nullable(),
  live_bindings: integer.nullable(), signalling_sessions: integer.nullable(), signalling_session_capacity: integer.nullable()
});
export const discoveryNodeSchema = z.strictObject({
  ...punchAssignmentSchema.shape,
  identity_version: integer.positive(), region: z.string().regex(/^[A-Za-z0-9][A-Za-z0-9_.-]{0,31}$/).nullable(),
  accepting_new_connections: z.boolean(), metrics: metricsSchema.nullable(), last_seen: integer, expires_at: integer
}).refine(node => node.expires_at > node.last_seen && node.expires_at <= node.last_seen + 90);
export const pageRequestSchema = z.strictObject({limit: z.number().int().min(1).max(200).default(200), after: nodeIdSchema.optional()});
export const discoveryPageSchema = z.object({
  version: z.literal(1), environment: environmentSchema, server_time: integer, expires_at: integer,
  nodes: z.array(discoveryNodeSchema).max(200), next_cursor: nodeIdSchema.nullable()
}).refine(page => page.expires_at > page.server_time && page.expires_at <= page.server_time + 60 &&
  page.nodes.every(node => node.last_seen <= page.server_time && node.expires_at >= page.expires_at));
export type DiscoveryPage = z.infer<typeof discoveryPageSchema>;
export type PageRequest = z.input<typeof pageRequestSchema>;
export interface PunchDiscoveryService {getPunches(input: PageRequest): Promise<DiscoveryPage>;}

/** Any supported endpoint with room is sufficient; WS/WSS share one capacity budget. */
export function acceptsNewPeer(node: {endpoints: readonly string[]; accepting_new_connections: boolean; metrics?: z.infer<typeof metricsSchema> | null}, schemes: readonly PunchScheme[]): boolean {
  if (!node.accepting_new_connections) return false;
  const full = (count: number | null, capacity: number | null) => count !== null && capacity !== null && count >= capacity;
  const metrics = node.metrics;
  return node.endpoints.some(endpoint => {
    const scheme = endpoint.slice(0, endpoint.indexOf(':')) as PunchScheme;
    if (!schemes.includes(scheme)) return false;
    return !metrics || (scheme === 'quic'
      ? !full(metrics.quic_connections, metrics.quic_connection_capacity)
      : !full(metrics.websocket_connections, metrics.websocket_connection_capacity));
  });
}
