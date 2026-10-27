import type { LayoutServerLoad } from './$types';
export const prerender = false;
export const load: LayoutServerLoad = ({ locals }) => ({ admin: locals.admin, locale: locals.locale });
