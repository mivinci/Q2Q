import { createRemoteJWKSet, jwtVerify, type JWTVerifyGetKey } from 'jose';

const keySets = new Map<string, JWTVerifyGetKey>();
export class AccessError extends Error {
  readonly status: 401 | 403 | 503;
  constructor(status: 401 | 403 | 503) { super('Admin access unavailable'); this.status=status; }
}

export async function verifyAccess(
  token: string | null,
  config: { ACCESS_TEAM_DOMAIN?: string; ACCESS_AUD?: string },
  testKeys?: JWTVerifyGetKey
): Promise<{ id: string; email: string }> {
  const team = config.ACCESS_TEAM_DOMAIN;
  const audience = config.ACCESS_AUD;
  if (!team || !/^[a-z0-9][a-z0-9-]*\.cloudflareaccess\.com$/.test(team) || !audience?.trim()) throw new AccessError(503);
  if (!token) throw new AccessError(401);
  const issuer = `https://${team}`;
  let keys = testKeys ?? keySets.get(issuer);
  if (!keys) {
    keys = createRemoteJWKSet(new URL(`${issuer}/cdn-cgi/access/certs`));
    // Configuration has one issuer; do not accumulate keys across config changes.
    keySets.clear(); keySets.set(issuer, keys);
  }
  try {
    const { payload } = await jwtVerify(token, keys, {
      issuer, audience, algorithms: ['RS256'], requiredClaims: ['sub', 'email', 'exp', 'iat'],
      clockTolerance: 5
    });
    if (!Number.isSafeInteger(payload.iat) || payload.iat! > Math.floor(Date.now()/1000)+5 || payload.type !== 'app' || typeof payload.sub !== 'string' || !payload.sub || typeof payload.email !== 'string' || !payload.email) throw new AccessError(403);
    return { id: `${issuer}#${payload.sub}`, email: payload.email };
  } catch (cause) {
    if (cause instanceof AccessError) throw cause;
    if (cause instanceof TypeError || (cause instanceof Error && cause.name === 'JWKSTimeout')) throw new AccessError(503);
    throw new AccessError(403);
  }
}
