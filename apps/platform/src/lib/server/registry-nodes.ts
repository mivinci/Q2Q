import { env } from 'cloudflare:workers';

type Metric = number | null;

export type NodeMetrics = {
  quic_connections: Metric;
  quic_connection_capacity: Metric;
  websocket_connections: Metric;
  websocket_connection_capacity: Metric;
  live_bindings: Metric;
  signalling_sessions: Metric;
  signalling_session_capacity: Metric;
};

export type RegistryNode = {
  node_id: string;
  service_type: 'punch' | 'track';
  region: string | null;
  online: boolean;
  accepting_new_connections: boolean;
  metrics: NodeMetrics | null;
  last_seen: number;
  expires_at: number;
};

export type RegistryNodesResult =
  | { state: 'ready'; server_time: number; nodes: RegistryNode[] }
  | { state: 'unconfigured' | 'unavailable'; server_time: null; nodes: [] };

type Page = { server_time: number; nodes: RegistryNode[]; next_cursor: string | null };

const nodeIdPattern = /^[A-Za-z0-9_.-]{1,128}$/;
const regionPattern = /^[A-Za-z0-9][A-Za-z0-9_.-]{0,31}$/;
const metricNames = [
  'quic_connections',
  'quic_connection_capacity',
  'websocket_connections',
  'websocket_connection_capacity',
  'live_bindings',
  'signalling_sessions',
  'signalling_session_capacity',
] as const;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isInteger(value: unknown): value is number {
  return Number.isSafeInteger(value) && (value as number) >= 0;
}

function parseMetrics(value: unknown): NodeMetrics | null | undefined {
  if (value === null) return null;
  if (!isRecord(value) || Object.keys(value).length !== metricNames.length) return undefined;
  if (!metricNames.every((name) => Object.hasOwn(value, name)
    && (value[name] === null || isInteger(value[name])))) return undefined;
  return Object.fromEntries(metricNames.map((name) => [name, value[name]])) as NodeMetrics;
}

function parsePage(value: unknown): Page | undefined {
  if (!isRecord(value) || !isInteger(value.server_time) || !Array.isArray(value.nodes)
    || value.nodes.length > 200
    || !(value.next_cursor === null || (typeof value.next_cursor === 'string' && nodeIdPattern.test(value.next_cursor)))) {
    return undefined;
  }
  const nodes: RegistryNode[] = [];
  for (const item of value.nodes) {
    if (!isRecord(item) || typeof item.node_id !== 'string' || !nodeIdPattern.test(item.node_id)
      || (item.service_type !== 'punch' && item.service_type !== 'track')
      || !(item.region === null || (typeof item.region === 'string' && regionPattern.test(item.region)))
      || typeof item.online !== 'boolean' || typeof item.accepting_new_connections !== 'boolean'
      || !isInteger(item.last_seen) || !isInteger(item.expires_at) || item.expires_at <= item.last_seen
      || item.online !== (item.expires_at > value.server_time)) {
      return undefined;
    }
    const metrics = parseMetrics(item.metrics);
    if (metrics === undefined || (item.service_type === 'track' && metrics !== null)) return undefined;
    nodes.push({
      node_id: item.node_id,
      service_type: item.service_type,
      region: item.region,
      online: item.online,
      accepting_new_connections: item.accepting_new_connections,
      metrics,
      last_seen: item.last_seen,
      expires_at: item.expires_at,
    });
  }
  return { server_time: value.server_time, nodes, next_cursor: value.next_cursor };
}

function nodesEndpoint(value: string): URL | undefined {
  try {
    const url = new URL(value);
    const host = url.hostname.replace(/^\[|\]$/g, '');
    const loopback = host === 'localhost' || host === '::1' || host === '127.0.0.1';
    if ((url.protocol !== 'https:' && !(url.protocol === 'http:' && loopback))
      || url.username || url.password || url.search || url.hash) return undefined;
    url.pathname = `${url.pathname.replace(/\/+$/, '')}/v1/nodes`;
    return url;
  } catch {
    return undefined;
  }
}

async function readJson(response: Response): Promise<unknown> {
  const reader = response.body?.getReader();
  if (!reader) throw new Error('missing response body');
  const chunks: Uint8Array[] = [];
  let size = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > 1024 * 1024) {
        void reader.cancel().catch(() => {});
        throw new Error('response too large');
      }
      chunks.push(value);
    }
  } finally {
    reader.releaseLock();
  }
  const bytes = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes));
}

async function fetchNodes(baseUrl: string, token: string): Promise<RegistryNodesResult> {
  const endpoint = nodesEndpoint(baseUrl);
  if (!endpoint || !token.trim() || token.length > 8192) {
    return { state: 'unavailable', server_time: null, nodes: [] };
  }
  const nodes: RegistryNode[] = [];
  const ids = new Set<string>();
  const cursors = new Set<string>();
  let cursor: string | null = null;
  let serverTime = 0;
  const signal = AbortSignal.timeout(10_000);
  for (let pageNumber = 0; pageNumber < 50; pageNumber++) {
    const url = new URL(endpoint);
    url.searchParams.set('limit', '200');
    if (cursor) url.searchParams.set('after', cursor);
    const response = await fetch(url, {
      headers: { Authorization: `Bearer ${token.trim()}` },
      cache: 'no-store',
      redirect: 'error',
      signal,
    });
    if (!response.ok) return { state: 'unavailable', server_time: null, nodes: [] };
    const page = parsePage(await readJson(response));
    if (!page) return { state: 'unavailable', server_time: null, nodes: [] };
    for (const node of page.nodes) {
      if (ids.has(node.node_id)) return { state: 'unavailable', server_time: null, nodes: [] };
      ids.add(node.node_id);
      nodes.push(node);
      if (nodes.length > 10_000) return { state: 'unavailable', server_time: null, nodes: [] };
    }
    serverTime = page.server_time;
    cursor = page.next_cursor;
    if (cursor === null) return { state: 'ready', server_time: serverTime, nodes };
    if (cursors.has(cursor)) return { state: 'unavailable', server_time: null, nodes: [] };
    cursors.add(cursor);
  }
  return { state: 'unavailable', server_time: null, nodes: [] };
}

export async function getRegistryNodes(): Promise<RegistryNodesResult> {
  const baseUrl = env.REGISTRY_URL;
  const token = env.REGISTRY_NODES_TOKEN;
  if (!baseUrl || !token) return { state: 'unconfigured', server_time: null, nodes: [] };
  try {
    return await fetchNodes(baseUrl, token);
  } catch {
    // Never include the Authorization header or an upstream error in page data/logs.
    return { state: 'unavailable', server_time: null, nodes: [] };
  }
}
