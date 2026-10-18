import { env } from 'cloudflare:workers';
import { fail, redirect } from '@sveltejs/kit';
import type { Actions, PageServerLoad } from './$types';
import { getProjectEntitlements } from '#lib/server/plans.js';
import {
  createProjectInvitation,
  invitationStatus,
  listProjectInvitations,
  revokeProjectInvitation
} from '#lib/server/project-invitations.js';
import { listProjectMembers, listProjects, projectCookie, removeProjectMember, renameProject, requireProject } from '#lib/server/projects.js';

async function selectedProject(userId: string, cookies: { get(name: string): string | undefined }) {
  const projects = await listProjects(env.DB, userId);
  const project = projects.find((item) => item.id === cookies.get(projectCookie)) ?? projects[0];
  if (!project) redirect(303, '/library');
  return requireProject(env.DB, userId, project.id);
}

export const load: PageServerLoad = async ({ locals, parent }) => {
  if (!locals.user) redirect(303, '/login');
  const { currentProject, entitlements } = await parent();
  if (!currentProject || !entitlements) redirect(303, '/library');

  const members = await listProjectMembers(env.DB, currentProject.id);
  const invitations = currentProject.role === 'owner'
    ? (await listProjectInvitations(env.DB, currentProject.id)).map((invitation) => ({
      ...invitation,
      status: invitationStatus(invitation)
    }))
    : [];
  return {
    project: currentProject, members, invitations,
    canInvite: entitlements.features.teamCollaboration && !currentProject.personalUserId,
    hasTeamCollaboration: entitlements.features.teamCollaboration,
    currentUserId: locals.user.id
  };
};

export const actions: Actions = {
  rename: async ({ locals, request, cookies }) => {
    if (!locals.user) redirect(303, '/login');
    const currentProject = await selectedProject(locals.user.id, cookies);
    await requireProject(env.DB, locals.user.id, currentProject.id, 'owner');
    if (currentProject.personalUserId) return fail(400, { error: 'personalProjectFixed' as const });

    const name = (await request.formData()).get('name');
    if (typeof name !== 'string' || !name.trim() || name.trim().length > 80) {
      return fail(400, { error: 'invalidProjectName' as const });
    }
    const saved = await renameProject(env.DB, currentProject.id, locals.user.id, name.trim());
    if (!saved) return fail(403, { error: 'projectActionFailed' as const });
    return { saved: true };
  },

  createInvite: async ({ locals, cookies, url }) => {
    if (!locals.user) redirect(303, '/login');
    const currentProject = await selectedProject(locals.user.id, cookies);
    const entitlements = await getProjectEntitlements(env.DB, currentProject.id);
    await requireProject(env.DB, locals.user.id, currentProject.id, 'owner');
    if (currentProject.personalUserId) return fail(400, { error: 'personalProjectFixed' as const });
    if (!entitlements.features.teamCollaboration) return fail(403, { error: 'teamPlanRequired' as const });

    const invite = await createProjectInvitation(env.DB, currentProject.id, locals.user.id);
    if (!invite) return fail(409, { error: 'inviteUnavailable' as const });
    // The raw bearer token is returned only once; only its hash is persisted.
    const inviteUrl = new URL(`/invites/${invite.token}`, url.origin).toString();
    return { inviteUrl, inviteExpiresAt: invite.expiresAt };
  },

  revokeInvite: async ({ locals, request, cookies }) => {
    if (!locals.user) redirect(303, '/login');
    const currentProject = await selectedProject(locals.user.id, cookies);
    await requireProject(env.DB, locals.user.id, currentProject.id, 'owner');
    const id = (await request.formData()).get('invitationId');
    if (typeof id !== 'string' || !id) return fail(400, { error: 'inviteUnavailable' as const });
    const revoked = await revokeProjectInvitation(env.DB, currentProject.id, locals.user.id, id);
    if (!revoked) return fail(409, { error: 'inviteUnavailable' as const });
    return { revoked: true };
  },

  removeMember: async ({ locals, request, cookies }) => {
    if (!locals.user) redirect(303, '/login');
    const currentProject = await selectedProject(locals.user.id, cookies);
    await requireProject(env.DB, locals.user.id, currentProject.id, 'owner');
    const userId = (await request.formData()).get('userId');
    if (typeof userId !== 'string' || !userId || userId === locals.user.id) {
      return fail(400, { error: 'memberActionFailed' as const });
    }
    const removed = await removeProjectMember(env.DB, currentProject.id, locals.user.id, userId);
    if (!removed) return fail(409, { error: 'memberActionFailed' as const });
    return { removed: true };
  }
};
