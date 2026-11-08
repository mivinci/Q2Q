import { z } from 'zod';

export const MAX_PUNCH_ENDPOINTS = 8;
export const MAX_PUNCH_ENDPOINT_LENGTH = 2048;
export const punchSchemeSchema = z.enum(['quic', 'wss', 'ws']);
export type PunchScheme = z.infer<typeof punchSchemeSchema>;

/** Canonical URLs are identity, not caller preference. Never include input in errors. */
export function canonicalPunchEndpoint(value: string): string | undefined {
  if (value.length > MAX_PUNCH_ENDPOINT_LENGTH || /[\x00-\x20\x7f\\#]/.test(value)) return;
  const match = /^(quic|wss|ws):\/\/([^/?#]*)(.*)$/i.exec(value);
  if (!match) return;
  const scheme = match[1]!.toLowerCase();
  const authority = match[2]!;
  if (!authority || authority.includes('@') || authority.endsWith(':')) return;
  try {
    // QUIC is not a WHATWG special scheme. Parse its authority using HTTPS too,
    // so IDNA, IPv4/IPv6 and port rules are identical to WebSocket in both runtimes.
    const url = new URL(`${scheme === 'quic' ? 'https' : scheme}://${authority}${match[3]}`);
    if (!url.hostname || url.username || url.password || url.hash) return;
    let result: string;
    if (scheme === 'quic') {
      const port = /:(\d+)$/.exec(authority)?.[1];
      if (!port || Number(port) < 1 || Number(port) > 65535 || url.pathname !== '/' || value.includes('?')) return;
      result = `quic://${url.hostname}:${Number(port)}`;
    } else {
      if (url.port === '0') return;
      if (scheme === 'ws' && url.hostname !== 'localhost' && url.hostname !== '[::1]' && !/^127\.\d+\.\d+\.\d+$/.test(url.hostname)) return;
      result = url.href;
    }
    if (result.length <= MAX_PUNCH_ENDPOINT_LENGTH) return result;
  } catch { /* Malformed authority or port; no network or DNS lookup. */ }
}

export const punchEndpointSchema = z.string().transform((value, ctx) => {
  const canonical = canonicalPunchEndpoint(value);
  if (canonical === undefined) {
    ctx.addIssue({ code: 'custom', message: 'Invalid Punch endpoint' });
    return z.NEVER;
  }
  return canonical;
});

/** Reject duplicates after URL canonicalization; never silently deduplicate signed identity. */
export const punchEndpointsSchema = z.array(punchEndpointSchema).min(1).max(MAX_PUNCH_ENDPOINTS)
  .refine(values => new Set(values).size === values.length, 'Duplicate Punch endpoint')
  .transform(values => values.sort());

/** Announce capability field: punch_schemes. It is not a preference order. */
export const punchSchemesSchema = z.array(punchSchemeSchema).min(1).max(3)
  .refine(values => new Set(values).size === values.length, 'Duplicate Punch scheme')
  .transform(values => values.sort());
