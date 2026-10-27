import { env } from 'cloudflare:workers';
import { error } from '@sveltejs/kit';
import { managementClient,reconcilePunchRequest,retryPunchRequest } from '#lib/server/registry-management.js';
import { mutationAction } from '#lib/server/mutation-action.js';
import { getAudit } from '#lib/server/audit.js';
import type { PageServerLoad,Actions } from './$types';
export const load:PageServerLoad=async({params,locals})=>{
 if(/^[a-f0-9-]{36}$/i.test(params.id))await reconcilePunchRequest(env.PLATFORM_DB,managementClient(env.REGISTRY_ADMIN),params.id);
 const record=await getAudit(env.PLATFORM_DB,params.id);
 return {record,canRetry:record.action==='punch.policy'&&record.status==='pending'&&record.actorId===locals.admin.id};
};
export const actions:Actions={retry:({request,url,locals,params})=>mutationAction(request,url,locals.admin,['requestId'],form=>{
 if(form.get('requestId')!==params.id)error(400,'Invalid request');
 return retryPunchRequest(env.PLATFORM_DB,managementClient(env.REGISTRY_ADMIN),locals.admin,params.id);
})};
