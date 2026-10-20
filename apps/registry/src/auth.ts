import { createMiddleware } from 'hono/factory';
import { createLocalJWKSet, errors, jwtVerify } from 'jose';
import { z } from 'zod';

const id = z.string().regex(/^[A-Za-z0-9_.-]{1,128}$/);
const integer = z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER);
const permission = z.enum(['nodes:write:self', 'discovery:punch:read', 'nodes:read:display']);
export type Permission = z.infer<typeof permission>;

const endpoint = (scheme: string) => z.string().refine(value => {
  try {
    const url = new URL(value);
    return url.protocol === `${scheme}:` && !!url.hostname && !url.username && !url.password
      && !url.search && !url.hash
      && (scheme !== 'quic' || (!!url.port && ['', '/'].includes(url.pathname)));
  } catch { return false; }
}, 'Invalid advertised endpoint');
const punchKey = z.string().regex(/^[A-Za-z0-9+/]{43}=$/).refine(value => {
  try { return atob(value).length === 32 && btoa(atob(value)) === value; }
  catch { return false; }
});
const common = z.object({
  iss: z.literal('q2q-registry'), aud: z.literal('q2q-registry'), sub: id,
  iat: integer, exp: integer, jti: z.string().min(1).max(128),
  scope: z.array(permission).min(1).max(3).refine(v => new Set(v).size === v.length),
});
const identity = { advertise_url: endpoint('https'), identity_version: integer.positive() };
const claimsSchema = z.discriminatedUnion('service_type', [
  common.extend({ service_type: z.literal('punch'), ...identity,
    advertise_url: endpoint('quic'), public_key: punchKey }),
  common.extend({ service_type: z.literal('track'), ...identity, public_key: z.never().optional() }),
  common.extend({ service_type: z.literal('platform'), advertise_url: z.never().optional(),
    public_key: z.never().optional(), identity_version: z.never().optional() }),
]).refine(v => v.exp > v.iat, 'Invalid validity interval').refine(v => {
  const allowed: Record<string, readonly Permission[]> = {
    punch: ['nodes:write:self'], track: ['nodes:write:self', 'discovery:punch:read'],
    platform: ['nodes:read:display'],
  };
  return v.scope.every(scope => allowed[v.service_type].includes(scope));
}, 'Permissions do not match service type');
export type Principal = z.infer<typeof claimsSchema>;

export class AuthError extends Error {
  readonly status = 401;
  readonly code: 'invalid_token' | 'token_expired';
  constructor(code: 'invalid_token' | 'token_expired' = 'invalid_token') {
    super(code === 'token_expired' ? 'Service token has expired' : 'Invalid service credentials');
    this.code = code;
  }
}
export class ForbiddenError extends Error {
  readonly status = 403;
  readonly code = 'forbidden';
  constructor() { super('Operation is not permitted'); }
}
export class RegistryConfigError extends Error {
  readonly status = 503;
  readonly code = 'registry_unavailable';
  constructor() { super('Registry authentication is unavailable'); }
}

const publicKeySchema = z.object({
  kty: z.literal('OKP'), crv: z.literal('Ed25519'), kid: id,
  x: z.string().regex(/^[A-Za-z0-9_-]{43}$/), d: z.never().optional(),
  alg: z.literal('EdDSA').optional(), use: z.literal('sig').optional(),
  key_ops: z.array(z.literal('verify')).optional(),
});

/** Only deployment-provided public keys are trusted; token URLs are never fetched. */
export function createVerifier(jwksJson: string) {
  let keys: ReturnType<typeof createLocalJWKSet>;
  try {
    const jwks = z.object({ keys: z.array(publicKeySchema).min(1).max(16) }).parse(JSON.parse(jwksJson));
    if (new Set(jwks.keys.map(k => k.kid)).size !== jwks.keys.length) throw new Error();
    keys = createLocalJWKSet(jwks);
  } catch { throw new RegistryConfigError(); }
  return async (authorization: string | undefined, now = new Date()): Promise<Principal> => {
    const match = authorization?.match(/^Bearer ([A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+)$/i);
    if (!match || match[1].length > 8192) throw new AuthError();
    try {
      const { payload, protectedHeader } = await jwtVerify(match[1], keys, {
        algorithms: ['EdDSA'], typ: 'JWT', issuer: 'q2q-registry', audience: 'q2q-registry',
        requiredClaims: ['sub', 'iat', 'exp', 'jti'], currentDate: now, clockTolerance: 30,
      });
      if (!id.safeParse(protectedHeader.kid).success) throw new AuthError();
      const result = claimsSchema.safeParse(payload);
      if (!result.success || result.data.iat > Math.floor(now.getTime() / 1000) + 30) throw new AuthError();
      // Authentication clock tolerance never extends the actual credential lease.
      if (result.data.exp <= Math.floor(now.getTime() / 1000)) throw new AuthError('token_expired');
      return result.data;
    } catch (error) {
      if (error instanceof AuthError) throw error;
      // jose verifies the signature before its claims checks, so forged expired
      // payloads cannot produce a trusted "expired" diagnosis.
      if (error instanceof errors.JWTExpired) throw new AuthError('token_expired');
      throw new AuthError();
    }
  };
}

export function authorize(principal: Principal, scope: Permission, nodeId?: string): void {
  if (!principal.scope.includes(scope) || (nodeId !== undefined && principal.sub !== nodeId)) {
    throw new ForbiddenError();
  }
}

export type RegistryEnv = {
  Bindings: { REGISTRY_JWKS: string; DB: D1Database };
  Variables: { principal: Principal };
};

/** Each middleware instance retains at most one deployment key set. */
export function authentication() {
  let cached: { json: string; verify: ReturnType<typeof createVerifier> } | undefined;
  return createMiddleware<RegistryEnv>(async (c, next) => {
    c.header('Cache-Control', 'no-store');
    if (!cached || cached.json !== c.env.REGISTRY_JWKS) {
      cached = { json: c.env.REGISTRY_JWKS, verify: createVerifier(c.env.REGISTRY_JWKS) };
    }
    c.set('principal', await cached.verify(c.req.header('Authorization')));
    await next();
  });
}

export function permissionRequired(scope: Permission) {
  return createMiddleware<RegistryEnv>(async (c, next) => {
    authorize(c.get('principal'), scope);
    await next();
  });
}

export { id as nodeIdSchema, punchKey as punchPublicKeySchema };
export const punchUrlSchema = endpoint('quic');
