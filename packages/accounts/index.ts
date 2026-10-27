export class AccountAccessError extends Error {
  constructor() { super('Account unavailable'); }
}

export async function prepareAccountSession(db: D1Database, session: { userId: string } & Record<string, unknown>) {
  const account = await db.prepare('SELECT disabledAt, authVersion FROM user WHERE id = ?')
    .bind(session.userId).first<{ disabledAt: number | null; authVersion: number }>();
  if (!account || account.disabledAt !== null) throw new AccountAccessError();
  // Never accept this version from the client or a stale user object.
  return { data: { ...session, authVersion: account.authVersion } };
}

export async function authorizeAccountSession(db: D1Database, sessionId: string, userId: string, now = Date.now()) {
  // Recheck persisted state even if the auth library returned a session earlier
  // in this request. The administrator may have revoked it in the meantime.
  return db.prepare(`SELECT u.id, u.name, u.image FROM user u JOIN session s ON s.userId = u.id
    WHERE s.id = ? AND u.id = ? AND ${sessionExpirySql('s')} > ? AND u.disabledAt IS NULL AND s.authVersion = u.authVersion`)
    .bind(sessionId, userId, now).first<{ id: string; name: string; image: string | null }>();
}

// Better Auth's SQLite adapter writes ISO text; older rows and local fixtures
// may contain epoch milliseconds despite the column's INTEGER affinity.
export function sessionExpirySql(alias: string): string {
  return `(CASE WHEN typeof(${alias}.expiresAt) IN ('integer', 'real') THEN ${alias}.expiresAt
    ELSE CAST(strftime('%s', ${alias}.expiresAt) AS INTEGER) * 1000
      + CAST(substr(strftime('%f', ${alias}.expiresAt), 4, 3) AS INTEGER) END)`;
}
