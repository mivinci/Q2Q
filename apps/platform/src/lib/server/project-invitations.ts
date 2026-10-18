import type { ProjectRole } from '#lib/server/projects.js';

export const pendingInviteCookie = 'q2q-pending-project-invite';
export const projectInviteLifetimeMs = 7 * 24 * 60 * 60 * 1000;
const maxPendingInvitations = 10;

export interface ProjectInvitation {
  id: string;
  createdAt: number;
  expiresAt: number;
  acceptedAt: number | null;
  revokedAt: number | null;
}

export interface ProjectInvitationDetails {
  id: string;
  projectId: string;
  projectName: string;
  inviterName: string;
  expiresAt: number;
  acceptedAt: number | null;
  acceptedBy: string | null;
  revokedAt: number | null;
  role: ProjectRole | null;
}

function tokenDigest(token: string): Promise<ArrayBuffer> {
  return crypto.subtle.digest('SHA-256', new TextEncoder().encode(token));
}

export async function hashProjectInviteToken(token: string): Promise<string> {
  const digest = new Uint8Array(await tokenDigest(token));
  return Array.from(digest, (byte) => byte.toString(16).padStart(2, '0')).join('');
}

function createToken(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(32));
  return btoa(String.fromCharCode(...bytes)).replaceAll('+', '-').replaceAll('/', '_').replace(/=+$/, '');
}

export function isProjectInviteToken(token: string): boolean {
  return /^[A-Za-z0-9_-]{43}$/.test(token);
}

export async function createProjectInvitation(
  db: D1Database, projectId: string, ownerId: string, now = Date.now()
): Promise<{ id: string; token: string; expiresAt: number } | null> {
  const id = crypto.randomUUID();
  const token = createToken();
  const tokenHash = await hashProjectInviteToken(token);
  const expiresAt = now + projectInviteLifetimeMs;
  const row = await db.prepare(`INSERT INTO project_invitation
    (id, projectId, tokenHash, createdBy, createdAt, expiresAt)
    SELECT ?, ?, ?, ?, ?, ?
    WHERE EXISTS (SELECT 1 FROM project_member WHERE projectId = ? AND userId = ? AND role = 'owner')
      AND EXISTS (SELECT 1 FROM project WHERE id = ? AND personalUserId IS NULL)
      AND EXISTS (SELECT 1 FROM project_subscription WHERE projectId = ? AND planId = 'team' AND status IN ('active', 'simulated'))
      AND (SELECT COUNT(*) FROM project_invitation WHERE projectId = ? AND acceptedAt IS NULL AND revokedAt IS NULL AND expiresAt > ?) < ?
    RETURNING id`).bind(id, projectId, tokenHash, ownerId, now, expiresAt,
    projectId, ownerId, projectId, projectId, projectId, now, maxPendingInvitations).first<{ id: string }>();
  return row ? { id, token, expiresAt } : null;
}

export async function listProjectInvitations(db: D1Database, projectId: string): Promise<ProjectInvitation[]> {
  const { results } = await db.prepare(`SELECT id, createdAt, expiresAt, acceptedAt, revokedAt
    FROM project_invitation WHERE projectId = ? ORDER BY createdAt DESC LIMIT 10`)
    .bind(projectId).all<ProjectInvitation>();
  return results;
}

export async function revokeProjectInvitation(
  db: D1Database, projectId: string, ownerId: string, invitationId: string, now = Date.now()
): Promise<boolean> {
  const row = await db.prepare(`UPDATE project_invitation SET revokedAt = ?
    WHERE id = ? AND projectId = ? AND acceptedAt IS NULL AND revokedAt IS NULL
      AND EXISTS (SELECT 1 FROM project_member WHERE projectId = ? AND userId = ? AND role = 'owner')
    RETURNING id`).bind(now, invitationId, projectId, projectId, ownerId).first<{ id: string }>();
  return row !== null;
}

export async function getProjectInvitationByToken(
  db: D1Database, tokenHash: string, userId: string | null
): Promise<ProjectInvitationDetails | null> {
  return db.prepare(`SELECT i.id, i.projectId, p.name AS projectName, u.name AS inviterName,
      i.expiresAt, i.acceptedAt, i.acceptedBy, i.revokedAt, m.role
    FROM project_invitation i JOIN project p ON p.id = i.projectId
    JOIN "user" u ON u.id = i.createdBy
    LEFT JOIN project_member m ON m.projectId = i.projectId AND m.userId = ?
    WHERE i.tokenHash = ?`).bind(userId, tokenHash).first<ProjectInvitationDetails>();
}

export function invitationStatus(invitation: ProjectInvitationDetails | ProjectInvitation, now = Date.now()): 'pending' | 'expired' | 'accepted' | 'revoked' {
  if (invitation.acceptedAt !== null) return 'accepted';
  if (invitation.revokedAt !== null) return 'revoked';
  if (invitation.expiresAt <= now) return 'expired';
  return 'pending';
}

export async function acceptProjectInvitation(
  db: D1Database, tokenHash: string, userId: string, now = Date.now()
): Promise<string | null> {
  const results = await db.batch([
    db.prepare(`UPDATE project_invitation SET acceptedAt = ?, acceptedBy = ?
      WHERE tokenHash = ? AND acceptedAt IS NULL AND revokedAt IS NULL AND expiresAt > ?
        AND EXISTS (SELECT 1 FROM project_subscription s WHERE s.projectId = project_invitation.projectId
          AND s.planId = 'team' AND s.status IN ('active', 'simulated'))
      RETURNING projectId`).bind(now, userId, tokenHash, now),
    db.prepare(`INSERT INTO project_member (projectId, userId, role, createdAt)
      SELECT projectId, ?, 'member', ? FROM project_invitation
      WHERE tokenHash = ? AND acceptedBy = ? AND acceptedAt = ?
      ON CONFLICT(projectId, userId) DO NOTHING
      RETURNING projectId`).bind(userId, now, tokenHash, userId, now)
  ]);
  const updated = results[0]?.results?.[0] as { projectId: string } | undefined;
  const added = results[1]?.results?.[0] as { projectId: string } | undefined;
  return updated?.projectId ?? added?.projectId ?? null;
}
