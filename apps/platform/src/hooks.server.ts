import type { Handle, HandleServerError } from '@sveltejs/kit/hooks';
import { env } from 'cloudflare:workers';
import { building } from '$app/env';
import { svelteKitHandler } from 'better-auth/svelte-kit';
import { error } from '@sveltejs/kit';
import { authorizeAccountSession } from '@q2q/accounts';
import { createAuth } from '#lib/server/auth.js';
import { localeCookie, resolveLocale } from '#lib/i18n.js';

export const handleError: HandleServerError = ({ event, kind }) => {
  if (kind !== 'unknown') return;
  // Do not log Request, URL or raw transport errors: grant and S3 URLs carry credentials.
  console.error('platform request failed', { route: event.route.id, kind });
  return { message: 'requestUnavailable' };
};

export const handle: Handle = async ({ event, resolve }) => {
  // Request-local data prevents language preferences leaking between SSR requests.
  event.locals.locale = resolveLocale(
    event.cookies.get(localeCookie), event.request.headers.get('accept-language') ?? ''
  );
  const auth = building ? null : createAuth(env);
  event.locals.authReady = auth !== null;
  event.locals.user = null;
  if (auth) {
    const session = await auth.api.getSession({ headers: event.request.headers });
    if (session) {
      event.locals.user = await authorizeAccountSession(env.DB, session.session.id, session.user.id);
      if (!event.locals.user && event.url.pathname.startsWith('/api/auth/') && event.url.pathname !== '/api/auth/sign-out') {
        error(403, 'Account session unavailable');
      }
    }
  } else if (event.url.pathname.startsWith('/api/auth/')) {
    error(503, 'Authentication is not configured');
  }
  const resolvePage: typeof resolve = (request) => resolve(request, {
    transformPageChunk: ({ html }) => html.replace('%q2q.locale%', event.locals.locale)
  });
  const result = auth
    ? await svelteKitHandler({ event, resolve: resolvePage, auth, building })
    : await resolvePage(event);
  const response = new Response(result.body, result);
  response.headers.set('Cache-Control', 'private, no-store');
  response.headers.set('Referrer-Policy', 'no-referrer');
  response.headers.set('Content-Language', event.locals.locale);
  // HTML depends on both the explicit preference and browser language.
  response.headers.append('Vary', 'Cookie, Accept-Language');
  return response;
};
