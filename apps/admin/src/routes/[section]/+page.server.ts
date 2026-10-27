import { env } from 'cloudflare:workers';
import { error } from '@sveltejs/kit';
import type { PageServerLoad } from './$types';
import { listRecords, plans, sections, type Section, type Column } from '#lib/server/queries.js';
import { listAudit } from '#lib/server/audit.js';
import { readManagedPunches,managementClient } from '#lib/server/registry-management.js';
export const load: PageServerLoad = async ({ params,url }) => {
 const section = params.section as Section;
 if (!sections.includes(section)) error(404,'Page not found');
 const base = {section,observedAt:Date.now(),rows:[] as Record<string,string|number|null>[],columns:[] as Column[],q:'',page:1,hasNext:false,unavailable:false,nextCursor:null as string|null,plans:[] as Awaited<ReturnType<typeof plans>>,audit:null as Awaited<ReturnType<typeof listAudit>> | null};
 if(section==='users'||section==='projects') {
  const columns:Column[] = section==='users' ? [{key:'name',label:'name'},{key:'email',label:'email'},{key:'accountStatus',label:'accountStatus',format:'label'},{key:'createdAt',label:'createdAt',format:'date'}] : [{key:'name',label:'name'},{key:'kind',label:'kind',format:'label'},{key:'members',label:'members'},{key:'planId',label:'planId',format:'label'}];
  return {...base,...await listRecords(env.PLATFORM_DB,section,url),columns};
 }
 if(section==='audit') return {...base,audit:await listAudit(env.PLATFORM_DB,url)};
 if(section==='plans') return {...base,plans:await plans(env.PLATFORM_DB)};
 if(section==='nodes') {
  const columns:Column[] = [{key:'node_id',label:'node_id'},{key:'policyStatus',label:'policyStatus',format:'label'},{key:'region',label:'region'},{key:'health',label:'health',format:'label'},{key:'accepting_new_connections',label:'accepting_new_connections',format:'label'},{key:'last_seen',label:'last_seen',format:'date'}];
  const cursor = url.searchParams.get('after');
  if(cursor && !/^[A-Za-z0-9_.-]{1,128}$/.test(cursor)) error(400,'Invalid cursor');
  try {const page = await readManagedPunches(managementClient(env.REGISTRY_ADMIN),cursor);return {...base,columns,rows:page.nodes.map(node=>({id:node.node_id,node_id:node.node_id,region:node.region,policyStatus:node.enabled?'discoverable':'discoveryStopped',health:node.online?'online':'offline',accepting_new_connections:node.accepting_new_connections?'yes':'no',last_seen:node.last_seen*1000})),nextCursor:page.next_cursor,observedAt:page.server_time*1000};}
  catch {return {...base,columns,unavailable:true};}
 }
 return base;
};
