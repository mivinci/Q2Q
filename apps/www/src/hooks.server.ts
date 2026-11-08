import type { Handle } from '@sveltejs/kit/hooks';
export const handle: Handle = async ({event,resolve}) => {
  const response = await resolve(event, {
    transformPageChunk: ({html}) => html.replace('%q2q.locale%',event.cookies.get('q2q-web-locale') === 'en' ? 'en' : 'zh-CN')
  });
  response.headers.set('Referrer-Policy','same-origin');
  response.headers.set('X-Content-Type-Options','nosniff');
  if (response.headers.get('content-type')?.includes('text/html')) response.headers.set('Cache-Control','private, no-store');
  return response;
};
