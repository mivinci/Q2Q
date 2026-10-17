import { env } from 'cloudflare:workers';
import type { LayoutServerLoad } from './$types';
import { ensurePersonalProject, listProjects } from '#lib/server/projects.js';

export const load: LayoutServerLoad = async ({ locals, cookies }) => {
  const user = locals.user;
  const base = { locale: locals.locale, user, authReady: locals.authReady };
  if (!user) return { ...base, projects: [], currentProject: null };

  let projects = await listProjects(env.DB, user.id);
  if (!projects.some((project) => project.personalUserId === user.id)) {
    await ensurePersonalProject(env.DB, user.id);
    projects = await listProjects(env.DB, user.id);
  }
  // The cookie only selects among projects whose membership was read from D1.
  const currentProject = projects.find((project) => project.id === cookies.get('q2q-platform-project')) ?? projects[0];
  return { ...base, projects, currentProject };
};
