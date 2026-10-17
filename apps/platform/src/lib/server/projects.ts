import { error } from '@sveltejs/kit';

export type ProjectRole = 'owner' | 'member';
export interface Project {
  id: string;
  name: string;
  personalUserId: string | null;
  role: ProjectRole;
}

export async function listProjects(db: D1Database, userId: string): Promise<Project[]> {
  const { results } = await db.prepare(`
    SELECT p.id, p.name, p.personalUserId, m.role
    FROM project p JOIN project_member m ON m.projectId = p.id
    WHERE m.userId = ? ORDER BY p.createdAt, p.id
  `).bind(userId).all<Project>();
  return results;
}

export async function requireProject(
  db: D1Database, userId: string, projectId: string, role?: ProjectRole
): Promise<Project> {
  const project = await db.prepare(`
    SELECT p.id, p.name, p.personalUserId, m.role
    FROM project p JOIN project_member m ON m.projectId = p.id
    WHERE p.id = ? AND m.userId = ?
  `).bind(projectId, userId).first<Project>();
  // Do not reveal whether a project exists to a non-member.
  if (!project) error(404, 'Project not found');
  if (role === 'owner' && project.role !== 'owner') error(403, 'Project owner required');
  return project;
}

export async function ensurePersonalProject(db: D1Database, userId: string): Promise<void> {
  const id = `personal:${userId}`;
  const now = Date.now();
  // Stable identity plus an atomic batch makes concurrent first requests safe.
  await db.batch([
    db.prepare(`INSERT INTO project (id, name, personalUserId, createdAt)
      VALUES (?, 'Personal', ?, ?) ON CONFLICT(personalUserId) DO NOTHING`).bind(id, userId, now),
    db.prepare(`INSERT INTO project_member (projectId, userId, role, createdAt)
      SELECT id, ?, 'owner', ? FROM project WHERE personalUserId = ?
      ON CONFLICT(projectId, userId) DO NOTHING`).bind(userId, now, userId)
  ]);
}

export async function createProject(db: D1Database, userId: string, name: string): Promise<string> {
  const id = crypto.randomUUID();
  const now = Date.now();
  // A failed owner insert rolls back the project insert too.
  await db.batch([
    db.prepare('INSERT INTO project (id, name, createdAt) VALUES (?, ?, ?)').bind(id, name, now),
    db.prepare(`INSERT INTO project_member (projectId, userId, role, createdAt)
      VALUES (?, ?, 'owner', ?)`).bind(id, userId, now)
  ]);
  return id;
}
