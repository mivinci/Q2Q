import type { Handle, HandleServerError } from '@sveltejs/kit/hooks';
import { env } from 'cloudflare:workers';
import { verifyAccess, AccessError } from '#lib/server/access.js';
import { resolveLocale } from '#lib/messages.js';

export const handle: Handle = async ({ event, resolve }) => {
  // Authentication is in the hook so child loaders and direct endpoints cannot skip it.
  try { event.locals.admin = await verifyAccess(event.request.headers.get('Cf-Access-Jwt-Assertion'), env); }
  catch (cause) {
    if (cause instanceof AccessError) return new Response('Admin access unavailable', {status:cause.status,headers:{'Cache-Control':'private, no-store','Content-Type':'text/plain; charset=utf-8','X-Content-Type-Options':'nosniff'}});
    throw cause;
  }
  event.locals.locale = resolveLocale(event.cookies.get('q2q-admin-locale'), event.request.headers.get('accept-language') ?? '');
  const result = await resolve(event, { transformPageChunk: ({ html }) => html.replace('%q2q.locale%', event.locals.locale) });
  const response = new Response(result.body, result);
  response.headers.set('Cache-Control', 'private, no-store');
  response.headers.set('Referrer-Policy', 'no-referrer');
  response.headers.set('X-Content-Type-Options', 'nosniff');
  response.headers.set('Content-Language', event.locals.locale);
  response.headers.append('Vary', 'Cookie, Accept-Language');
  return response;
};
export const handleError: HandleServerError = ({ event }) => {
  console.error('admin request failed', { route: event.route.id });
  return { message: 'Admin request unavailable' };
};
