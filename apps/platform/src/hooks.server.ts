import type { Handle } from '@sveltejs/kit/hooks';
import { localeCookie, resolveLocale } from '#lib/i18n.js';

export const handle: Handle = async ({ event, resolve }) => {
  // Request-local data prevents language preferences leaking between SSR requests.
  event.locals.locale = resolveLocale(
    event.cookies.get(localeCookie), event.request.headers.get('accept-language') ?? ''
  );
  const response = await resolve(event, {
    transformPageChunk: ({ html }) => html.replace('%q2q.locale%', event.locals.locale)
  });
  response.headers.set('Content-Language', event.locals.locale);
  // HTML depends on both the explicit preference and browser language.
  response.headers.append('Vary', 'Cookie, Accept-Language');
  return response;
};
