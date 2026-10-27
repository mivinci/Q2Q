import { error } from '@sveltejs/kit';

export const auditActions = ['plan.update', 'subscription.set', 'user.disable', 'user.restore', 'user.sessions.revoke', 'punch.policy'] as const;
export interface AuditRecord {
  requestId: string; actorId: string; actorEmail: string; action: typeof auditActions[number];
  status: 'applied' | 'pending' | 'rejected';
  requestedEnabled?: number;
  targetId: string; reason: string; beforeJson: string; afterJson: string; revision: number; createdAt: number;
}
export async function getAudit(db: D1Database, id: string) {
  if (!/^[a-f0-9]{8}(-[a-f0-9]{4}){3}-[a-f0-9]{12}$/i.test(id)) error(404, 'Record not found');
  const record = await db.prepare(`SELECT requestId, actorId, actorEmail, action, targetId, reason,
    beforeJson, afterJson, revision, createdAt, 'applied' AS status FROM admin_operation WHERE requestId = ? AND appliedAt IS NOT NULL`)
    .bind(id).first<AuditRecord>();
  if (record) return record;
  const remote = await db.prepare(`SELECT requestId, actorId, actorEmail, 'punch.policy' AS action, targetId, reason,
    COALESCE(json_extract(resultJson,'$.operation.before'),'null') AS beforeJson,
    COALESCE(json_extract(resultJson,'$.operation.after'),'null') AS afterJson,
    COALESCE(json_extract(resultJson,'$.operation.after.revision'),0) AS revision, createdAt,status,json_extract(payloadJson,'$.enabled') AS requestedEnabled FROM registry_request WHERE requestId=?`).bind(id).first<AuditRecord>();
  if(!remote) error(404, 'Record not found');
  return remote;
}
function day(value: string | null, end = false): number | null {
  if (!value) return null;
  if (!/^\d{4}-\d{2}-\d{2}$/.test(value)) error(400, 'Invalid date filter');
  const time = Date.parse(`${value}T00:00:00Z`);
  if (!Number.isFinite(time) || new Date(time).toISOString().slice(0, 10) !== value) error(400, 'Invalid date filter');
  return time + (end ? 86400000 : 0);
}
export async function listAudit(db: D1Database, url: URL) {
  const q = (url.searchParams.get('q') ?? '').trim();
  const action = url.searchParams.get('action') ?? '';
  const page = Number(url.searchParams.get('page') ?? 1);
  const from = day(url.searchParams.get('from'));
  const to = day(url.searchParams.get('to'), true);
  if (q.length > 200 || !['', ...auditActions].includes(action) ||
      !Number.isSafeInteger(page) || page < 1 || page > 4000 || (from !== null && to !== null && to <= from)) error(400, 'Invalid audit filter');
  const { results } = await db.prepare(`SELECT requestId, actorEmail, action, targetId, createdAt,status FROM (
    SELECT requestId,actorEmail,action,targetId,actorId,createdAt,'applied' AS status FROM admin_operation WHERE appliedAt IS NOT NULL
    UNION ALL SELECT requestId,actorEmail,'punch.policy' AS action,targetId,actorId,createdAt,status FROM registry_request
    ) WHERE 1=1
      AND (? = '' OR instr(lower(actorEmail), lower(?)) > 0 OR instr(actorId, ?) > 0 OR instr(targetId, ?) > 0 OR requestId = ?)
      AND (? = '' OR action = ?) AND (? IS NULL OR createdAt >= ?) AND (? IS NULL OR createdAt < ?)
    ORDER BY createdAt DESC, requestId LIMIT 26 OFFSET ?`)
    .bind(q, q, q, q, q, action, action, from, from, to, to, (page - 1) * 25)
    .all<Pick<AuditRecord, 'requestId' | 'actorEmail' | 'action' | 'targetId' | 'createdAt' | 'status'>>();
  return { rows: results.slice(0, 25), hasNext: results.length > 25, page, q, action, from: url.searchParams.get('from') ?? '', to: url.searchParams.get('to') ?? '' };
}
