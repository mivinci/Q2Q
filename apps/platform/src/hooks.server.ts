import type { Handle, HandleServerError } from '@sveltejs/kit/hooks';
import { env } from 'cloudflare:workers';
import { building } from '$app/env';
import { svelteKitHandler } from 'better-auth/svelte-kit';
import { error } from '@sveltejs/kit';
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
      // Only expose the display identity; session tokens stay in the auth boundary.
      event.locals.user = { id: session.user.id, name: session.user.name, image: session.user.image ?? null };
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
  // Same-origin HTML form POSTs need a non-opaque Origin for SvelteKit's CSRF
  // check. `no-referrer` makes browsers send `Origin: null` on those requests.
  response.headers.set('Referrer-Policy', 'same-origin');
  response.headers.set('Content-Language', event.locals.locale);
  // HTML depends on both the explicit preference and browser language.
  response.headers.append('Vary', 'Cookie, Accept-Language');
  return response;
};
