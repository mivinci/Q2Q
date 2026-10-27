import { APIError } from 'better-auth/api';
import { prepareAccountSession, AccountAccessError } from '@q2q/accounts';
import { betterAuth } from 'better-auth';
import { sveltekitCookies } from 'better-auth/svelte-kit';
import { getRequestEvent } from '$app/server';

export interface AuthEnvironment {
  DB: D1Database;
  AUTH_ORIGIN?: string;
  BETTER_AUTH_SECRET?: string;
  GITHUB_CLIENT_ID?: string;
  GITHUB_CLIENT_SECRET?: string;
  GOOGLE_CLIENT_ID?: string;
  GOOGLE_CLIENT_SECRET?: string;
}

export function authConfigured(env: AuthEnvironment | undefined): env is AuthEnvironment & {
  AUTH_ORIGIN: string; BETTER_AUTH_SECRET: string; GITHUB_CLIENT_ID: string; GITHUB_CLIENT_SECRET: string;
} {
  return !!(env?.DB && env.AUTH_ORIGIN && env.BETTER_AUTH_SECRET &&
    env.BETTER_AUTH_SECRET.length >= 32 && env.GITHUB_CLIENT_ID && env.GITHUB_CLIENT_SECRET);
}

export function createAuth(env: Parameters<typeof authConfigured>[0]) {
  if (!authConfigured(env)) return null;
  // D1 bindings belong to the current request; do not cache the auth instance globally.
  return betterAuth({
    appName: 'Q2Q',
    baseURL: env.AUTH_ORIGIN,
    secret: env.BETTER_AUTH_SECRET,
    database: env.DB,
    trustedOrigins: [new URL(env.AUTH_ORIGIN).origin],
    socialProviders: {
      github: { clientId: env.GITHUB_CLIENT_ID, clientSecret: env.GITHUB_CLIENT_SECRET },
      ...(env.GOOGLE_CLIENT_ID && env.GOOGLE_CLIENT_SECRET
        ? { google: { clientId: env.GOOGLE_CLIENT_ID, clientSecret: env.GOOGLE_CLIENT_SECRET } }
        : {})
    },
    account: { encryptOAuthTokens: true, accountLinking: { enabled: false } },
    session: {
      expiresIn: 60 * 60 * 24 * 7, updateAge: 60 * 60 * 24,
      cookieCache: { enabled: false },
      additionalFields: { authVersion: { type: 'number', required: true, defaultValue: 1, input: false, returned: false } }
    },
    databaseHooks: { session: { create: { before: async session => {
      try { return await prepareAccountSession(env.DB, session); }
      catch (cause) {
        if (cause instanceof AccountAccessError) throw new APIError('FORBIDDEN', { message: 'Account unavailable' });
        throw cause;
      }
    } } } },
    rateLimit: { enabled: true, storage: 'database', window: 60, max: 60 },
    advanced: { ipAddress: { ipAddressHeaders: ['cf-connecting-ip'] } },
    telemetry: { enabled: false },
    plugins: [sveltekitCookies(getRequestEvent)]
  });
}
