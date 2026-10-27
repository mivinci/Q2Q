import { env } from 'cloudflare:workers';
import { error } from '@sveltejs/kit';
import { plans } from '#lib/server/queries.js';
import { setSubscription, subscriptionFields } from '#lib/server/mutations.js';
import { mutationAction } from '#lib/server/mutation-action.js';
import type { PageServerLoad, Actions } from './$types';
import { manageUser, userFields } from '#lib/server/user-mutations.js';
import { details } from '#lib/server/queries.js';
export const load: PageServerLoad = async ({params,url}) => {
 if(params.section!=='users' && params.section!=='projects') error(404,'Page not found');
 const definitions = params.section === 'projects' ? await plans(env.PLATFORM_DB) : [];
 const subscription = params.section === 'projects' ? await env.PLATFORM_DB.prepare('SELECT planId, periodStartAt, periodEndAt, revision FROM project_subscription WHERE projectId = ?').bind(params.id).first<{planId:string;periodStartAt:number|null;periodEndAt:number|null;revision:number}>() : null;
 return {subscription,definitions,requestId:crypto.randomUUID(),revokeRequestId:crypto.randomUUID(),section:params.section,...await details(env.PLATFORM_DB,params.section,params.id,url),observedAt:Date.now()};
};

export const actions: Actions = {
 default: ({request,url,locals,params}) => {
  if(params.section === 'users') return mutationAction(request,url,locals.admin,userFields,form => manageUser(env.PLATFORM_DB,locals.admin,params.id,form));
  if(params.section !== 'projects') error(404, 'Page not found');
  return mutationAction(request,url,locals.admin,subscriptionFields,
    form => setSubscription(env.PLATFORM_DB,locals.admin,params.id,form));
 }
};
