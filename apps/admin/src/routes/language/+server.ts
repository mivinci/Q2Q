import { error, redirect } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
export const POST: RequestHandler = async ({ request, url, cookies }) => {
 if (request.headers.get('origin') !== url.origin) error(403, 'Invalid origin');
 const form = await request.formData(); const locale = form.get('locale'); const returnTo = form.get('returnTo');
 if (locale !== 'en' && locale !== 'zh-CN') error(400, 'Invalid locale');
 cookies.set('q2q-admin-locale', locale, { path: '/', httpOnly: true, sameSite: 'lax', secure: url.protocol === 'https:', maxAge: 31536000 });
 const target = typeof returnTo === 'string' ? new URL(returnTo, url.origin) : new URL('/',url);
 redirect(303, target.origin === url.origin && target.pathname !== '/language' ? target.pathname + target.search : '/');
};
