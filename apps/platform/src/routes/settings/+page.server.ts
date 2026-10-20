import { env } from 'cloudflare:workers';
import { redirect } from '@sveltejs/kit';
import type { PageServerLoad } from './$types';

type AccountProfile = { name: string; email: string; image: string | null };
type LoginProvider = { providerId: string };

export const load: PageServerLoad = async ({ locals }) => {
  if (!locals.user) redirect(303, '/login');

  const [account, providers] = await Promise.all([
    env.DB.prepare('SELECT name, email, image FROM "user" WHERE id = ?1')
      .bind(locals.user.id).first<AccountProfile>(),
    env.DB.prepare('SELECT DISTINCT providerId FROM account WHERE userId = ?1 ORDER BY providerId')
      .bind(locals.user.id).all<LoginProvider>()
  ]);

  return {
    account: account ? { ...account, providers: providers.results.map(({ providerId }) => providerId) } : null
  };
};
