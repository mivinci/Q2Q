import { error, redirect } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import { isLocale, localeCookie } from '#lib/i18n.js';

export const POST: RequestHandler = async ({ request, cookies, url }) => {
  const locale = (await request.formData()).get('locale');
  if (!isLocale(locale)) error(400, 'Unsupported locale');
  cookies.set(localeCookie, locale, {
    path: '/', maxAge: 60 * 60 * 24 * 365, httpOnly: true,
    sameSite: 'lax', secure: url.protocol === 'https:'
  });
  redirect(303, '/');
};
