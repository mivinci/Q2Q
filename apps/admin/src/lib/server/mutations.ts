import { getPlanCatalog, isPlanId, planFeatures, uploadMaxSize, effectivePlanIdSql } from '@q2q/plans';

export type MutationCode = 'invalidChanges' | 'changeConflict' | 'requestConflict' | 'recordMissing';
export class MutationError extends Error {
  readonly code: MutationCode;
  readonly status: 400 | 404 | 409;
  constructor(code: MutationCode, status: 400 | 404 | 409 = 400) {
    super(code); this.code = code; this.status = status;
  }
}
export interface Actor { id: string; email: string }
interface StoredOperation {
  requestId: string; actorId: string; fingerprint: string; revision: number;
  createdAt: number; appliedAt: number | null;
}
export interface SavedOperation { requestId: string; revision: number; createdAt: number; replayed: boolean }
const uuid = /^[a-f0-9]{8}(-[a-f0-9]{4}){3}-[a-f0-9]{12}$/i;
export const planFields = ['requestId', 'revision', 'reason', 'maxFileSizeBytes', 'storageBytes', ...planFeatures];
export const subscriptionFields = ['requestId', 'revision', 'reason', 'planId', 'planRevisions', 'startAt', 'endAt'];

function field(form: FormData, key: string): string {
  const values = form.getAll(key);
  if (values.length !== 1 || typeof values[0] !== 'string') throw new MutationError('invalidChanges');
  return values[0];
}
function integer(value: string, min: number, max: number): number {
  if (!/^\d+$/.test(value)) throw new MutationError('invalidChanges');
  const number = Number(value);
  if (!Number.isSafeInteger(number) || number < min || number > max) throw new MutationError('invalidChanges');
  return number;
}
function base(form: FormData, allowed: string[]) {
  for (const key of form.keys()) if (!allowed.includes(key)) throw new MutationError('invalidChanges');
  const requestId = field(form, 'requestId');
  const revision = integer(field(form, 'revision'), 0, Number.MAX_SAFE_INTEGER - 1);
  const reason = field(form, 'reason').trim();
  if (!uuid.test(requestId) || !reason || reason.length > 1000) throw new MutationError('invalidChanges');
  return { requestId, revision, reason };
}
function timestamp(value: string): number | null {
  if (!value) return null;
  if (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(:\d{2}(\.\d{1,3})?)?$/.test(value)) throw new MutationError('invalidChanges');
  const result = Date.parse(`${value}Z`);
  if (!Number.isSafeInteger(result) || result < 0 || new Date(result).toISOString().slice(0, 10) !== value.slice(0, 10)) throw new MutationError('invalidChanges');
  return result;
}
async function fingerprint(value: unknown): Promise<string> {
  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(JSON.stringify(value)));
  return Array.from(new Uint8Array(digest), byte => byte.toString(16).padStart(2, '0')).join('');
}
function saved(row: StoredOperation, actor: Actor, hash: string, replayed: boolean): SavedOperation {
  if (row.actorId !== actor.id || row.fingerprint !== hash) throw new MutationError('requestConflict', 409);
  if (row.appliedAt === null) throw Error('Operation result not confirmed');
  return { requestId: row.requestId, revision: row.revision, createdAt: row.createdAt, replayed };
}
async function replay(db: D1Database, requestId: string, actor: Actor, hash: string) {
  if (await db.prepare('SELECT requestId FROM registry_request WHERE requestId=?').bind(requestId).first()) throw new MutationError('requestConflict',409);
  const row = await db.prepare(`SELECT requestId, actorId, fingerprint, revision, createdAt, appliedAt
    FROM admin_operation WHERE requestId = ? AND appliedAt IS NOT NULL`).bind(requestId).first<StoredOperation>();
  return row ? saved(row, actor, hash, true) : null;
}
// These guards match the exact operation inserted in this transaction. Replays
// cannot mutate again, and a reused request ID with different content cannot act.
const pending = `EXISTS (SELECT 1 FROM admin_operation WHERE requestId = ? AND actorId = ? AND fingerprint = ? AND appliedAt IS NULL)`;
async function commit(
  db: D1Database, actor: Actor, hash: string, requestId: string,
  statements: D1PreparedStatement[], now: number
): Promise<SavedOperation> {
  const results = await db.batch([
    ...statements,
    db.prepare(`UPDATE admin_operation SET appliedAt = ? WHERE requestId = ? AND actorId = ? AND fingerprint = ? AND appliedAt IS NULL`)
      .bind(now, requestId, actor.id, hash),
    db.prepare(`SELECT requestId, actorId, fingerprint, revision, createdAt, appliedAt FROM admin_operation WHERE requestId = ?`).bind(requestId)
  ]);
  const row = results.at(-1)?.results?.[0] as StoredOperation | undefined;
  if (!row) throw new MutationError('changeConflict', 409);
  return saved(row, actor, hash, results[0].meta.changes === 0);
}
const planSnapshot = (alias: string) => `json_object('maxFileSizeBytes', ${alias}.maxFileSizeBytes, 'storageBytes', ${alias}.storageBytes,
  'platformStorage', ${alias}.platformStorage, 'platformNetwork', ${alias}.platformNetwork,
  'customTrack', ${alias}.customTrack, 'higherLimits', ${alias}.higherLimits,
  'teamCollaboration', ${alias}.teamCollaboration, 'revision', ${alias}.revision)`;

export async function updatePlan(db: D1Database, actor: Actor, id: string, form: FormData): Promise<SavedOperation> {
  if (!isPlanId(id)) throw new MutationError('recordMissing', 404);
  const input = base(form, planFields);
  if (input.revision < 1) throw new MutationError('invalidChanges');
  const maxFileSizeBytes = integer(field(form, 'maxFileSizeBytes'), 1, uploadMaxSize);
  const storageBytes = integer(field(form, 'storageBytes'), maxFileSizeBytes, Number.MAX_SAFE_INTEGER);
  const flags = planFeatures.map(feature => {
    if (!form.has(feature)) return 0;
    if (field(form, feature) !== 'on') throw new MutationError('invalidChanges');
    return 1;
  });
  const next = { maxFileSizeBytes, storageBytes, ...Object.fromEntries(planFeatures.map((key, i) => [key, flags[i]])), revision: input.revision + 1 };
  const hash = await fingerprint({ action: 'plan.update', target: id, ...input, next });
  const existing = await replay(db, input.requestId, actor, hash);
  if (existing) return existing;
  await getPlanCatalog(db);
  const now = Date.now();
  const count = `(SELECT COUNT(*) FROM project p WHERE ${effectivePlanIdSql('p.id', String(now))} = catalog.id)`;
  return commit(db, actor, hash, input.requestId, [
    db.prepare(`INSERT INTO admin_operation
      (requestId, actorId, actorEmail, action, targetId, reason, fingerprint, beforeJson, afterJson, revision, createdAt)
      SELECT ?, ?, ?, 'plan.update', ?, ?, ?, ${planSnapshot('catalog')}, json_set(?, '$.affectedProjects', ${count}), ?, ?
      FROM plan_catalog catalog WHERE id = ? AND revision = ? ON CONFLICT(requestId) DO NOTHING`)
      .bind(input.requestId, actor.id, actor.email, id, input.reason, hash, JSON.stringify(next), next.revision, now, id, input.revision),
    db.prepare(`UPDATE plan_catalog SET maxFileSizeBytes = ?, storageBytes = ?,
      platformStorage = ?, platformNetwork = ?, customTrack = ?, higherLimits = ?, teamCollaboration = ?,
      revision = revision + 1, updatedAt = ? WHERE id = ? AND revision = ? AND ${pending}`)
      .bind(maxFileSizeBytes, storageBytes, ...flags, now, id, input.revision, input.requestId, actor.id, hash)
  ], now);
}

export async function setSubscription(db: D1Database, actor: Actor, projectId: string, form: FormData): Promise<SavedOperation> {
  const input = base(form, subscriptionFields);
  const planId = field(form, 'planId');
  if (!isPlanId(planId)) throw new MutationError('invalidChanges');
  const start = timestamp(field(form, 'startAt'));
  const end = timestamp(field(form, 'endAt'));
  if (end !== null && start !== null && end <= start) throw new MutationError('invalidChanges');
  let revisions: unknown;
  try { revisions = JSON.parse(field(form, 'planRevisions')); } catch { throw new MutationError('invalidChanges'); }
  if (!revisions || typeof revisions !== 'object') throw new MutationError('invalidChanges');
  const planRevision = (revisions as Record<string, unknown>)[planId];
  const freeRevision = (revisions as Record<string, unknown>).free;
  if (!Number.isSafeInteger(freeRevision) || Number(freeRevision) < 1 || !Number.isSafeInteger(planRevision) || Number(planRevision) < 1) throw new MutationError('invalidChanges');
  const next = { planId, status: 'active', provider: 'manual', periodStartAt: start, periodEndAt: end, cancelAtPeriodEnd: 0, revision: input.revision + 1 };
  const hash = await fingerprint({ action: 'subscription.set', target: projectId, ...input, next, planRevision, freeRevision });
  const existing = await replay(db, input.requestId, actor, hash);
  if (existing) return existing;
  await getPlanCatalog(db);
  if (!await db.prepare('SELECT id FROM project WHERE id = ?').bind(projectId).first()) throw new MutationError('recordMissing', 404);
  const now = Date.now();
  return commit(db, actor, hash, input.requestId, [
    db.prepare(`INSERT INTO admin_operation
      (requestId, actorId, actorEmail, action, targetId, reason, fingerprint, beforeJson, afterJson, revision, createdAt)
      SELECT ?, ?, ?, 'subscription.set', ?, ?, ?,
        CASE WHEN s.projectId IS NULL THEN 'null' ELSE json_object('planId', s.planId, 'status', s.status,
          'provider', s.provider, 'periodStartAt', s.periodStartAt, 'periodEndAt', s.periodEndAt,
          'cancelAtPeriodEnd', s.cancelAtPeriodEnd, 'revision', s.revision) END,
        json_set(?, '$.planSnapshot', ${planSnapshot('catalog')}), ?, ?
      FROM project p LEFT JOIN project_subscription s ON s.projectId = p.id
      JOIN plan_catalog catalog ON catalog.id = ? AND catalog.revision = ?
      JOIN plan_catalog fallback ON fallback.id = 'free' AND fallback.revision = ?
      WHERE p.id = ? AND COALESCE(s.revision, 0) = ?
        AND COALESCE(s.provider, '') != 'creem'
        AND NOT EXISTS (SELECT 1 FROM billing_checkout ci WHERE ci.projectId = p.id AND ci.status IN ('creating','open','unknown'))
      ON CONFLICT(requestId) DO NOTHING`)
      .bind(input.requestId, actor.id, actor.email, projectId, input.reason, hash, JSON.stringify(next), next.revision, now, planId, planRevision, freeRevision, projectId, input.revision),
    db.prepare(`INSERT INTO project_subscription
      (projectId, planId, status, provider, periodStartAt, periodEndAt, cancelAtPeriodEnd, createdAt, updatedAt, revision)
      SELECT ?, ?, 'active', 'manual', ?, ?, 0, ?, ?, ? WHERE ${pending}
      ON CONFLICT(projectId) DO UPDATE SET planId = excluded.planId, status = 'active', provider = 'manual',
        providerSubscriptionId = NULL, periodStartAt = excluded.periodStartAt, periodEndAt = excluded.periodEndAt,
        cancelAtPeriodEnd = 0, updatedAt = excluded.updatedAt, revision = excluded.revision
      WHERE project_subscription.revision = ?`)
      .bind(projectId, planId, start, end, now, now, next.revision, input.requestId, actor.id, hash, input.revision)
  ], now);
}

export { base as parseOperation, fingerprint, replay, pending, commit };
