import { sessionExpirySql } from '@q2q/accounts';
import { error } from '@sveltejs/kit';
import { getProjectEntitlements, getPlanCatalog, PlanConfigurationError, activeSubscriptionSql, planIds } from '@q2q/plans';
import type { MessageKey } from '../messages.js';

async function catalog(db: D1Database) {
 try { return await getPlanCatalog(db); }
 catch (cause) { if (cause instanceof PlanConfigurationError) error(503, 'Plan catalog unavailable'); throw cause; }
}

export type Row = Record<string, string | number | null>;
export interface Column { key: string; label: MessageKey; format?: 'date' | 'bytes' | 'label' }
export const sections = ['users', 'projects', 'plans', 'nodes', 'audit'] as const;
export type Section = typeof sections[number];
export function pagination(url: URL) {
 const q = (url.searchParams.get('q') ?? '').trim();
 const page = Number(url.searchParams.get('page') ?? '1');
 if (q.length > 100 || !Number.isSafeInteger(page) || page < 1 || page > 4000) error(400, 'Invalid search or page');
 return { q, page, offset: (page - 1) * 25 };
}
export async function listRecords(db: D1Database, section: 'users' | 'projects', url: URL) {
 const {q, page, offset} = pagination(url);
 const now = Date.now();
 if (section === 'projects') await catalog(db);
 const query = section === 'users'
  ? `SELECT id, name, email, createdAt, CASE WHEN disabledAt IS NULL THEN 'enabled' ELSE 'disabled' END AS accountStatus FROM user WHERE ? = '' OR instr(lower(name), lower(?)) > 0 OR instr(lower(email), lower(?)) > 0 OR instr(id, ?) > 0 ORDER BY id LIMIT 26 OFFSET ?`
  : `SELECT p.id, p.name, CASE WHEN p.personalUserId IS NULL THEN 'teamProject' ELSE 'personal' END AS kind,
    (SELECT COUNT(*) FROM project_member WHERE projectId = p.id) AS members,
    CASE WHEN ${activeSubscriptionSql('s', '?')} THEN s.planId ELSE 'free' END AS planId
    FROM project p LEFT JOIN project_subscription s ON s.projectId = p.id
    WHERE ? = '' OR instr(lower(p.name), lower(?)) > 0 OR instr(p.id, ?) > 0 ORDER BY p.id LIMIT 26 OFFSET ?`;
 const args = section === 'users' ? [q,q,q,q,offset] : [now,now,q,q,q,offset];
 const {results} = await db.prepare(query).bind(...args).all<Row>();
 return { rows: results.slice(0,25), hasNext: results.length > 25, q, page };
}
export async function overview(db: D1Database) {
 await catalog(db);
 const now = Date.now();
 const [users, projects, plans] = await db.batch<{count:number;planId?:string}>([
  db.prepare('SELECT COUNT(*) AS count FROM user'),
  db.prepare('SELECT COUNT(*) AS count FROM project'),
  db.prepare(`SELECT CASE WHEN ${activeSubscriptionSql('s', '?')} THEN s.planId ELSE 'free' END AS planId, COUNT(*) AS count
   FROM project p LEFT JOIN project_subscription s ON s.projectId = p.id GROUP BY 1`).bind(now,now)
 ]);
 return {users: Number(users.results[0]?.count ?? 0), projects: Number(projects.results[0]?.count ?? 0),
  plans: planIds.map(id => ({id,count:Number(plans.results.find(row => row.planId === id)?.count ?? 0)}))};
}
export async function details(db: D1Database, section: 'users' | 'projects', id: string, url: URL): Promise<{record:Row;fields:Column[];related:Row[];page:number;hasNext:boolean;relatedSection:string;columns:Column[]}> {
 const {page,offset} = pagination(url);
 const now = Date.now();
 if (id.length > 200) error(404,'Record not found');
 if (section === 'users') {
  const user = await db.prepare(`SELECT id, name, email, createdAt, disabledAt, disabledReason, accessRevision, CASE WHEN disabledAt IS NULL THEN 'enabled' ELSE 'disabled' END AS accountStatus FROM user WHERE id = ?`).bind(id).first<Row>();
  if (!user) error(404,'Record not found');
  const sessions = await db.prepare(`SELECT COUNT(*) AS count FROM session s JOIN user u ON u.id = s.userId WHERE u.id = ? AND u.disabledAt IS NULL AND ${sessionExpirySql('s')} > ? AND s.authVersion = u.authVersion`).bind(id, now).first<{count:number}>();
  const owners = await db.prepare("SELECT COUNT(*) AS count FROM project_member WHERE userId = ? AND role = 'owner'").bind(id).first<{count:number}>();
  const {results: providers} = await db.prepare('SELECT DISTINCT providerId FROM account WHERE userId = ? ORDER BY providerId').bind(id).all<{providerId:string}>();
  const {results: projects} = await db.prepare(`SELECT p.id, p.name, m.role, CASE WHEN ${activeSubscriptionSql('s', '?')} THEN s.planId ELSE 'free' END AS planId
   FROM project_member m JOIN project p ON p.id = m.projectId LEFT JOIN project_subscription s ON s.projectId = p.id
   WHERE m.userId = ? ORDER BY p.id LIMIT 26 OFFSET ?`).bind(now,now,id,offset).all<Row>();
  return { record: {...user, activeSessions:sessions?.count ?? 0, ownerProjects:owners?.count ?? 0, providerId: providers.map(p=>p.providerId).join(', ')}, fields: [{key:'id',label:'id'},{key:'name',label:'name'},{key:'email',label:'email'},{key:'createdAt',label:'createdAt',format:'date'},{key:'providerId',label:'providerId'},{key:'accountStatus',label:'accountStatus',format:'label'},{key:'disabledAt',label:'disabledAt',format:'date'},{key:'disabledReason',label:'disabledReason'},{key:'activeSessions',label:'activeSessions'},{key:'ownerProjects',label:'ownerProjects'}] satisfies Column[],
   related: projects.slice(0,25), page, hasNext:projects.length > 25, relatedSection:'projects', columns:[{key:'name',label:'name'},{key:'role',label:'role',format:'label'},{key:'planId',label:'planId',format:'label'}] satisfies Column[] };
 }
 const project = await db.prepare('SELECT id, name, createdAt FROM project WHERE id = ?').bind(id).first<Row>();
 if (!project) error(404,'Record not found');
 const subscription = await db.prepare('SELECT planId, status, provider, periodStartAt, periodEndAt, cancelAtPeriodEnd FROM project_subscription WHERE projectId = ?').bind(id).first<{planId:string;status:string;provider:string|null;periodStartAt:number|null;periodEndAt:number|null;cancelAtPeriodEnd:number}>();
 const entitlements = await getProjectEntitlements(db,id,now).catch(cause => { if (cause instanceof PlanConfigurationError) error(503, 'Plan catalog unavailable'); throw cause; });
 const usage = await db.prepare(`SELECT
  COALESCE(SUM(CASE WHEN t.status = 'uploaded' THEN t.size ELSE 0 END),0) AS usedBytes,
  COALESCE(SUM(CASE WHEN t.status <> 'uploaded' THEN t.size ELSE 0 END),0) AS reservedBytes
  FROM upload_task t JOIN file_record f ON f.id = t.fileId JOIN storage_connection c ON c.id = t.connectionId
  WHERE t.projectId = ? AND c.provider = 'platform-s3' AND f.status <> 'deleted'
  AND (t.status IN ('initializing','uploading','completing','uploaded') OR t.cleanupPending = 1)`).bind(id).first<{usedBytes:number;reservedBytes:number}>();
 const {results: members} = await db.prepare(`SELECT u.id, u.name, u.email, m.role FROM project_member m JOIN user u ON u.id = m.userId WHERE m.projectId = ? ORDER BY u.id LIMIT 26 OFFSET ?`).bind(id,offset).all<Row>();
 return {record:{...project, effectivePlan:entitlements.planId, rawStatus:subscription?.status ?? 'free',source:subscription?.provider ?? null,
   periodStartAt:subscription?.periodStartAt ?? null,periodEndAt:subscription?.periodEndAt ?? null,
   usedBytes:usage?.usedBytes ?? 0,reservedBytes:usage?.reservedBytes ?? 0,storageBytes:entitlements.limits.storageBytes},
  fields:[{key:'id',label:'id'},{key:'name',label:'name'},{key:'createdAt',label:'createdAt',format:'date'},{key:'effectivePlan',label:'effectivePlan',format:'label'},{key:'rawStatus',label:'rawStatus',format:'label'},{key:'source',label:'source',format:'label'},{key:'periodStartAt',label:'periodStartAt',format:'date'},{key:'periodEndAt',label:'periodEndAt',format:'date'},{key:'usedBytes',label:'usedBytes',format:'bytes'},{key:'reservedBytes',label:'reservedBytes',format:'bytes'},{key:'storageBytes',label:'storageBytes',format:'bytes'}] satisfies Column[],
  related:members.slice(0,25),page,hasNext:members.length>25,relatedSection:'users',columns:[{key:'name',label:'name'},{key:'email',label:'email'},{key:'role',label:'role',format:'label'}] satisfies Column[]};
}
export async function plans(db: D1Database) { const definitions = await catalog(db); return planIds.map(id=>({id,...definitions[id]})); }
