import type { Context } from 'hono';
import { z } from 'zod';
import { metricsSchema, discoveryNodeSchema, pageRequestSchema } from '@q2q/punch-discovery';
export { metricsSchema } from '@q2q/punch-discovery';
import { AuthError, ForbiddenError, nodeIdSchema } from './auth.ts';
import type { RegistryEnv } from './auth.ts';

const integer = z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER);
const label = z.string().regex(/^[A-Za-z0-9][A-Za-z0-9_.-]{0,31}$/);
const reportSchema = z.strictObject({
  version: z.string().regex(/^[A-Za-z0-9][A-Za-z0-9.+_-]{0,63}$/),
  region: label.nullable().optional().default(null),
  accepting_new_connections: z.boolean(),
  metrics: metricsSchema.nullable(),
});
const querySchema = z.strictObject({
  limit: z.string().regex(/^[1-9][0-9]{0,2}$/).transform(Number).pipe(z.number().max(200)).optional().default(100),
  after: nodeIdSchema.optional(),
});
const rowSchema = z.object({
  node_id: nodeIdSchema,
  service_type: z.enum(['punch', 'track']),
  identity_version: integer.positive(), advertise_url: z.string(), public_key: z.string().nullable(),
  discovery_enabled: z.union([z.literal(0),z.literal(1)]).optional().default(1),
  version: z.string(), region: label.nullable(), accepting_new_connections: z.union([z.literal(0), z.literal(1)]),
  metrics_json: z.string().nullable(), created_at: integer, last_seen: integer, expires_at: integer,
}).refine(row => row.created_at <= row.last_seen && row.expires_at > row.last_seen && row.expires_at <= row.last_seen + 90);
const leaseSchema = z.object({ node_id: nodeIdSchema, last_seen: integer, expires_at: integer });
const displayNode = z.object({
  node_id: nodeIdSchema, service_type: z.enum(['punch', 'track']), region: label.nullable(), online: z.boolean(),
  accepting_new_connections: z.boolean(), metrics: metricsSchema.nullable(), last_seen: integer, expires_at: integer,
});

export class RegistryError extends Error {
  readonly code: string;
  readonly status: 400 | 409 | 413 | 415 | 429 | 503;
  constructor(status: RegistryError['status'], code: string, detail: string) {
    super(detail);
    this.status = status;
    this.code = code;
  }
}
function invalid(detail = 'Invalid request fields'): never {
  throw new RegistryError(400, 'invalid_request', detail);
}
function unavailable(): never {
  throw new RegistryError(503, 'registry_unavailable', 'Registry storage is unavailable');
}

/** Bound actual bytes, including chunked bodies and dishonest Content-Length. */
async function readJson(c: Context<RegistryEnv>): Promise<unknown> {
  if (c.req.header('Content-Type')?.split(';')[0].trim().toLowerCase() !== 'application/json'
    || !['identity', undefined].includes(c.req.header('Content-Encoding'))) {
    throw new RegistryError(415, 'unsupported_media_type', 'Expected uncompressed application/json');
  }
  const max = 16 * 1024;
  const length = c.req.header('Content-Length');
  if (length && (!/^[0-9]+$/.test(length) || !Number.isSafeInteger(Number(length)))) invalid('Invalid Content-Length');
  if (length && Number(length) > max) throw new RegistryError(413, 'payload_too_large', 'Report exceeds 16 KiB');
  const reader = c.req.raw.body?.getReader();
  if (!reader) invalid('JSON body is required');
  const chunks: Uint8Array[] = [];
  let size = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > max) {
        void reader.cancel().catch(() => {});
        throw new RegistryError(413, 'payload_too_large', 'Report exceeds 16 KiB');
      }
      chunks.push(value);
    }
  } catch (error) {
    if (error instanceof RegistryError) throw error;
    invalid('Unable to read JSON body');
  } finally { reader.releaseLock(); }
  const bytes = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
  try { return JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes)); }
  catch { invalid('Malformed JSON body'); }
}

async function first<T>(statement: D1PreparedStatement): Promise<T | null> {
  try { return await statement.first<T>(); } catch { unavailable(); }
}

// GCRA: each admitted attempt adds six seconds of debt; a 12-second
// tolerance allows three attempts at once. One conditional D1 upsert shares the
// budget across isolates, IPs, signing tokens and identity revisions of an ID.
const takePermit = `INSERT INTO report_limits (node_id, theoretical_arrival)
SELECT ?, unixepoch() + 6 WHERE ? > unixepoch()
ON CONFLICT(node_id) DO UPDATE SET
  theoretical_arrival = MAX(report_limits.theoretical_arrival, unixepoch()) + 6
WHERE report_limits.theoretical_arrival <= unixepoch() + 12
RETURNING theoretical_arrival`;

async function limitReport(c: Context<RegistryEnv>, nodeId: string, tokenExpiry: number) {
  let permit: { theoretical_arrival: number } | null;
  try {
    permit = await first(c.env.DB.prepare(takePermit).bind(nodeId, tokenExpiry));
  } catch { unavailable(); }
  if (permit) return;
  let state: { server_time: number; retry_after: number } | null;
  try {
    state = await first(c.env.DB.prepare(`SELECT unixepoch() AS server_time,
      MAX(1, COALESCE(report_limits.theoretical_arrival, 0) - unixepoch() - 12) AS retry_after
      FROM (SELECT 1) LEFT JOIN report_limits ON node_id = ?`).bind(nodeId));
  } catch { unavailable(); }
  if (!state) unavailable();
  if (tokenExpiry <= state.server_time) throw new AuthError('token_expired');
  const retry = integer.positive().safeParse(state.retry_after);
  if (!retry.success) unavailable();
  c.header('Retry-After', String(retry.data));
  throw new RegistryError(429, 'rate_limited', 'Node report rate exceeded; retry after the indicated delay');
}

// The conflict predicate and write run in one SQLite statement. A pre-read would
// allow a concurrent old credential to overwrite a newer identity revision.
const upsert = `INSERT INTO nodes (
  node_id, service_type, identity_version, advertise_url, public_key,
  version, region, accepting_new_connections, metrics_json, created_at, last_seen, expires_at
) SELECT ?, ?, ?, ?, ?, ?, ?, ?, ?, unixepoch(), unixepoch(), MIN(unixepoch() + 90, ?)
WHERE ? > unixepoch()
ON CONFLICT(node_id) DO UPDATE SET
  identity_version = excluded.identity_version,
  advertise_url = excluded.advertise_url, public_key = excluded.public_key,
  version = excluded.version, region = excluded.region,
  accepting_new_connections = excluded.accepting_new_connections,
  metrics_json = excluded.metrics_json, last_seen = excluded.last_seen, expires_at = excluded.expires_at
WHERE nodes.service_type = excluded.service_type AND (
  excluded.identity_version > nodes.identity_version OR (
    excluded.identity_version = nodes.identity_version AND
    excluded.advertise_url = nodes.advertise_url AND excluded.public_key IS nodes.public_key
  )
)
RETURNING node_id, last_seen, expires_at`;

export async function report(c: Context<RegistryEnv>) {
  const principal = c.get('principal');
  if (principal.service_type === 'platform') throw new ForbiddenError();
  const parsed = reportSchema.safeParse(await readJson(c));
  if (!parsed.success) invalid('Invalid report fields or metrics');
  const body = parsed.data;
  if (principal.service_type === 'track' && body.metrics !== null) invalid('Track metrics must be null');
  await limitReport(c, principal.sub, principal.exp);
  let statement: D1PreparedStatement;
  try {
    statement = c.env.DB.prepare(upsert).bind(
      principal.sub, principal.service_type, principal.identity_version, principal.advertise_url,
      principal.service_type === 'punch' ? principal.public_key : null,
      body.version, body.region, Number(body.accepting_new_connections),
      body.metrics === null ? null : JSON.stringify(body.metrics), principal.exp, principal.exp,
    );
  } catch { unavailable(); }
  const result = await first<unknown>(statement);
  if (result) {
    const lease = leaseSchema.safeParse(result);
    if (!lease.success) unavailable();
    return c.json({ ...lease.data, report_interval_seconds: 30 });
  }
  // Read the database clock after a rejected write, including queue-time expiry.
  let current: { identity_version: number | null; service_type: string | null; server_time: number } | null;
  try {
    current = await first(c.env.DB.prepare(`SELECT unixepoch() AS server_time,
      nodes.identity_version, nodes.service_type FROM (SELECT 1)
      LEFT JOIN nodes ON nodes.node_id = ?`).bind(principal.sub));
  } catch { unavailable(); }
  if (!current) unavailable();
  if (principal.exp <= current.server_time) throw new AuthError('token_expired');
  if (current?.service_type === principal.service_type && (current.identity_version ?? 0) > principal.identity_version) {
    throw new RegistryError(409, 'stale_identity', 'A newer node identity is already registered');
  }
  throw new RegistryError(409, 'identity_conflict', 'Node type or identity description conflicts with its registered identity');
}

function pagination(c: Context<RegistryEnv>) {
  const parameters = new URL(c.req.url).searchParams;
  for (const name of parameters.keys()) {
    if (parameters.getAll(name).length !== 1) invalid('Repeated pagination parameters');
  }
  const parsed = querySchema.safeParse(Object.fromEntries(parameters));
  if (!parsed.success) invalid('Invalid pagination parameters');
  return parsed.data;
}
function storedMetrics(row: z.infer<typeof rowSchema>) {
  try {
    const metrics = row.metrics_json === null ? null : metricsSchema.parse(JSON.parse(row.metrics_json));
    if (row.service_type === 'track' && metrics !== null) unavailable();
    return metrics;
  } catch { unavailable(); }
}

async function rows(db: D1Database, parameters: {limit:number; after?:string}, discovery: boolean, serverTime: number) {
  const { limit, after } = parameters;
  let result: D1Result<unknown>;
  try {
    const where = discovery ? "service_type = 'punch' AND expires_at > ? AND node_id > ?" : 'node_id > ?';
    const args = discovery ? [serverTime, after ?? '', limit + 1] : [after ?? '', limit + 1];
    result = await db.prepare(`SELECT ${discovery ? 'nodes.*, CASE WHEN EXISTS(SELECT 1 FROM punch_policy p WHERE p.node_id=nodes.node_id AND p.enabled=0) THEN 0 ELSE 1 END AS discovery_enabled' : '*'} FROM nodes WHERE ${where} ORDER BY node_id COLLATE BINARY LIMIT ?`).bind(...args).all();
    if (!result.success) unavailable();
  } catch { unavailable(); }
  const parsed = z.array(rowSchema).safeParse(result.results);
  if (!parsed.success) unavailable();
  const page = parsed.data.slice(0, limit);
  return { page, next_cursor: parsed.data.length > limit ? page[page.length - 1].node_id : null };
}

export async function discoverPage(db: D1Database, input: unknown) {
  const request = pageRequestSchema.safeParse(input);
  if (!request.success) invalid();
  const serverTime = Math.floor(Date.now() / 1000);
  const { page, next_cursor } = await rows(db, request.data, true, serverTime);
  const parsed = z.array(discoveryNodeSchema).safeParse(page.map(row => ({
    punch_id: row.node_id, url: row.advertise_url, public_key: row.public_key,
    identity_version: row.identity_version, region: row.region,
    accepting_new_connections: Boolean(row.accepting_new_connections) && row.discovery_enabled === 1, metrics: storedMetrics(row),
    last_seen: row.last_seen, expires_at: row.expires_at,
  })));
  if (!parsed.success) unavailable();
  return { server_time: serverTime, nodes: parsed.data, next_cursor };
}

export async function discover(c: Context<RegistryEnv>) {
  return c.json(await discoverPage(c.env.DB, pagination(c)));
}

export async function display(c: Context<RegistryEnv>) {
  const serverTime = Math.floor(Date.now() / 1000);
  const { page, next_cursor } = await rows(c.env.DB, pagination(c), false, serverTime);
  // Explicitly build the display DTO. Never serialize a D1 row or JWT wholesale.
  const nodes = z.array(displayNode).parse(page.map(row => ({
    node_id: row.node_id, service_type: row.service_type, region: row.region,
    online: row.expires_at > serverTime, accepting_new_connections: Boolean(row.accepting_new_connections),
    metrics: storedMetrics(row), last_seen: row.last_seen, expires_at: row.expires_at,
  })));
  return c.json({ server_time: serverTime, nodes, next_cursor });
}
