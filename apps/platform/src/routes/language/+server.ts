import { error, redirect } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import { isLocale, localeCookie } from '#lib/i18n.js';

export const POST: RequestHandler = async ({ request, cookies, url }) => {
  const form = await request.formData();
  const locale = form.get('locale');
  if (!isLocale(locale)) error(400, 'Unsupported locale');
  cookies.set(localeCookie, locale, {
    path: '/', maxAge: 60 * 60 * 24 * 365, httpOnly: true,
    sameSite: 'lax', secure: url.protocol === 'https:'
  });
  const returnTo = form.get('returnTo');
  // Only return to known local pages; never redirect to a supplied external URL.
  redirect(303, returnTo === '/' || returnTo === '/settings' || returnTo === '/login' || returnTo === '/storage' || returnTo === '/seeds' || returnTo === '/network' || returnTo === '/usage' || returnTo === '/project/settings' ? returnTo : '/library');
};
