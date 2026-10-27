import { MutationError, parseOperation, fingerprint, replay, pending, commit, type Actor } from './mutations.js';

export const userFields = ['requestId', 'revision', 'reason', 'mode'];
const actions = { disable: 'user.disable', restore: 'user.restore', revoke: 'user.sessions.revoke' } as const;
export async function manageUser(db: D1Database, actor: Actor, userId: string, form: FormData) {
  const input = parseOperation(form, userFields);
  const modes = form.getAll('mode');
  if (input.revision < 1 || modes.length !== 1 || typeof modes[0] !== 'string' || !Object.hasOwn(actions, modes[0])) throw new MutationError('invalidChanges');
  const mode = modes[0] as keyof typeof actions;
  const action = actions[mode];
  const hash = await fingerprint({ action, target: userId, ...input });
  const existing = await replay(db, input.requestId, actor, hash);
  if (existing) return existing;
  if (!await db.prepare('SELECT id FROM user WHERE id = ?').bind(userId).first()) throw new MutationError('recordMissing', 404);
  const now = Date.now();
  const allowed = mode === 'disable' ? 'u.disabledAt IS NULL' : mode === 'restore' ? 'u.disabledAt IS NOT NULL' : '1 = 1';
  const afterDisabledAt = mode === 'disable' ? String(now) : mode === 'restore' ? 'NULL' : 'u.disabledAt';
  const afterReason = mode === 'disable' ? '?' : mode === 'restore' ? 'NULL' : 'u.disabledReason';
  const args: (string | number)[] = [input.requestId, actor.id, actor.email, action, userId, input.reason, hash];
  if (mode === 'disable') args.push(input.reason);
  args.push(input.revision + 1, now, userId, input.revision);
  return commit(db, actor, hash, input.requestId, [
    db.prepare(`INSERT INTO admin_operation
      (requestId, actorId, actorEmail, action, targetId, reason, fingerprint, beforeJson, afterJson, revision, createdAt)
      SELECT ?, ?, ?, ?, ?, ?, ?,
        json_object('disabledAt', u.disabledAt, 'disabledReason', u.disabledReason,
          'accessRevision', u.accessRevision, 'authVersion', u.authVersion,
          'storedSessions', (SELECT COUNT(*) FROM session WHERE userId = u.id)),
        json_object('disabledAt', ${afterDisabledAt}, 'disabledReason', ${afterReason},
          'accessRevision', u.accessRevision + 1, 'authVersion', u.authVersion + 1, 'storedSessions', 0), ?, ?
      FROM user u WHERE u.id = ? AND u.accessRevision = ? AND ${allowed} AND u.authVersion < 9007199254740991
      ON CONFLICT(requestId) DO NOTHING`).bind(...args),
    db.prepare(`UPDATE user SET disabledAt = ${mode === 'disable' ? '?' : mode === 'restore' ? 'NULL' : 'disabledAt'},
      disabledReason = ${mode === 'disable' ? '?' : mode === 'restore' ? 'NULL' : 'disabledReason'},
      authVersion = authVersion + 1, accessRevision = accessRevision + 1, updatedAt = ?
      WHERE id = ? AND accessRevision = ? AND ${pending}`)
      .bind(...(mode === 'disable' ? [now, input.reason] : []), now, userId, input.revision, input.requestId, actor.id, hash),
    db.prepare(`DELETE FROM session WHERE userId = ? AND ${pending}`)
      .bind(userId, input.requestId, actor.id, hash)
  ], now);
}
