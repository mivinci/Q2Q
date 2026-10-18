import { env } from 'cloudflare:workers';
import { fail, redirect } from '@sveltejs/kit';
import type { Actions, PageServerLoad } from './$types';
import { getProjectEntitlements } from '#lib/server/plans.js';
import {
  acceptProjectInvitation,
  getProjectInvitationByToken,
  hashProjectInviteToken,
  invitationStatus,
  isProjectInviteToken,
  pendingInviteCookie
} from '#lib/server/project-invitations.js';
import { projectCookie } from '#lib/server/projects.js';

async function invitationFor(token: string, userId: string | null) {
  if (!isProjectInviteToken(token)) return null;
  return getProjectInvitationByToken(env.DB, await hashProjectInviteToken(token), userId);
}

export const load: PageServerLoad = async ({ locals, params, cookies, url }) => {
  if (!locals.user && isProjectInviteToken(params.token)) {
    cookies.set(pendingInviteCookie, params.token, {
      path: '/', httpOnly: true, sameSite: 'lax', secure: url.protocol === 'https:', maxAge: 10 * 60
    });
  }
  const invite = await invitationFor(params.token, locals.user?.id ?? null);
  if (!invite) return { invitation: null, status: 'invalid' as const, role: null, canAccept: false, acceptedByCurrentUser: false, user: locals.user };

  const status = invitationStatus(invite);
  if (locals.user && invite.acceptedBy === locals.user.id) {
    cookies.set(projectCookie, invite.projectId, {
      path: '/', sameSite: 'lax', secure: url.protocol === 'https:', maxAge: 60 * 60 * 24 * 365
    });
  }
  const entitlements = locals.user && status === 'pending' && !invite.role
    ? await getProjectEntitlements(env.DB, invite.projectId)
    : null;
  return {
    invitation: { projectName: invite.projectName, inviterName: invite.inviterName, expiresAt: invite.expiresAt },
    status,
    role: invite.role,
    acceptedByCurrentUser: !!locals.user && invite.acceptedBy === locals.user.id,
    canAccept: !!locals.user && !invite.role && status === 'pending' && !!entitlements?.features.teamCollaboration,
    user: locals.user
  };
};

export const actions: Actions = {
  accept: async ({ locals, params, cookies, url }) => {
    if (!locals.user) redirect(303, '/login');
    const invite = await invitationFor(params.token, locals.user.id);
    if (!invite) return fail(404, { error: 'inviteInvalid' as const });

    const status = invitationStatus(invite);
    if (status !== 'pending') {
      if (status === 'accepted' && invite.acceptedBy === locals.user.id) {
        cookies.set(projectCookie, invite.projectId, {
          path: '/', sameSite: 'lax', secure: url.protocol === 'https:', maxAge: 60 * 60 * 24 * 365
        });
        redirect(303, '/library');
      }
      const errorCode: 'inviteExpired' | 'inviteRevokedPublic' | 'inviteAlreadyAccepted' =
        status === 'expired' ? 'inviteExpired' : status === 'revoked' ? 'inviteRevokedPublic' : 'inviteAlreadyAccepted';
      return fail(409, { error: errorCode });
    }

    if (invite.role) {
      cookies.set(projectCookie, invite.projectId, {
        path: '/', sameSite: 'lax', secure: url.protocol === 'https:', maxAge: 60 * 60 * 24 * 365
      });
      redirect(303, '/library');
    }
    const entitlements = await getProjectEntitlements(env.DB, invite.projectId);
    if (!entitlements.features.teamCollaboration) return fail(403, { error: 'teamPlanRequired' as const });

    const projectId = await acceptProjectInvitation(env.DB, await hashProjectInviteToken(params.token), locals.user.id);
    if (!projectId) return fail(409, { error: 'inviteUnavailable' as const });
    cookies.set(projectCookie, projectId, {
      path: '/', sameSite: 'lax', secure: url.protocol === 'https:', maxAge: 60 * 60 * 24 * 365
    });
    redirect(303, '/library');
  }
};
