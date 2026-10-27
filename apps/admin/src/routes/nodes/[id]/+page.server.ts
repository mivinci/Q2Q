import { env } from 'cloudflare:workers';
import { error } from '@sveltejs/kit';
import { managementClient,readManagedPunch,updatePunchPolicy,punchFields } from '#lib/server/registry-management.js';
import { mutationAction } from '#lib/server/mutation-action.js';
import type { PageServerLoad,Actions } from './$types';
export const load:PageServerLoad=async({params})=>{
 if(!/^[A-Za-z0-9_.-]{1,128}$/.test(params.id))error(404,'Node not found');
 let node;
 try{node=await readManagedPunch(managementClient(env.REGISTRY_ADMIN),params.id);}catch{return {node:null,nodeId:params.id,unavailable:true,requestId:crypto.randomUUID(),observedAt:Date.now()};}
 if(!node)error(404,'Node not found');
 return {node,nodeId:params.id,unavailable:false,requestId:crypto.randomUUID(),observedAt:Date.now()};
};
export const actions:Actions={default:({request,url,locals,params})=>mutationAction(request,url,locals.admin,punchFields,
 form=>updatePunchPolicy(env.PLATFORM_DB,managementClient(env.REGISTRY_ADMIN),locals.admin,params.id,form))};
