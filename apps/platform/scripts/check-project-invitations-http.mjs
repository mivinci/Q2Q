// End-to-end HTTP checks against a local platform server and its local D1 database.
// Test users, sessions, project and invitations are always removed in finally.
import assert from 'node:assert/strict';
import { createHash, createHmac, randomUUID } from 'node:crypto';
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';

const baseUrl = new URL(process.env.Q2Q_PLATFORM_BASE_URL ?? 'http://localhost:5174');
assert.ok(['localhost', '127.0.0.1', '::1'].includes(baseUrl.hostname), 'HTTP checks are restricted to localhost');
assert.equal(baseUrl.protocol, 'http:', 'HTTP checks require a local HTTP dev server');

const appDir = new URL('../', import.meta.url).pathname;
const devVars = Object.fromEntries(readFileSync(join(appDir, '.dev.vars'), 'utf8')
  .split(/\r?\n/).filter((line) => line && !line.trimStart().startsWith('#'))
  .map((line) => {
    const separator = line.indexOf('=');
    return separator < 0 ? [line.trim(), ''] : [line.slice(0, separator).trim(), line.slice(separator + 1).trim()];
  }));
assert.ok(devVars.BETTER_AUTH_SECRET?.length >= 32, 'BETTER_AUTH_SECRET must be configured in apps/platform/.dev.vars');

const d1Dir = join(appDir, '.wrangler/state/v3/d1/miniflare-D1DatabaseObject');
const databaseFile = readdirSync(d1Dir).find((file) => file.endsWith('.sqlite') && file !== 'metadata.sqlite');
assert.ok(databaseFile, 'local D1 database file not found');
const sql = new DatabaseSync(join(d1Dir, databaseFile));

const suffix = randomUUID();
const ownerId = `invite-http-owner-${suffix}`;
const memberId = `invite-http-member-${suffix}`;
const projectId = `invite-http-project-${suffix}`;
const ownerToken = `invite-http-owner-session-${suffix}`;
const memberToken = `invite-http-member-session-${suffix}`;
const now = Date.now();
const expiresAt = now + 60 * 60 * 1000;
const checks = [];

function insertAccount(id, name, token) {
  sql.prepare(`INSERT INTO "user" (id, name, email, emailVerified, createdAt, updatedAt)
    VALUES (?, ?, ?, 1, ?, ?)`).run(id, name, `${id}@example.test`, now, now);
  sql.prepare(`INSERT INTO session (id, expiresAt, token, createdAt, updatedAt, userId)
    VALUES (?, ?, ?, ?, ?, ?)`).run(randomUUID(), expiresAt, token, now, now, id);
}

function signedCookie(token) {
  const signature = createHmac('sha256', devVars.BETTER_AUTH_SECRET).update(token).digest('base64');
  return `better-auth.session_token=${encodeURIComponent(`${token}.${signature}`)}`;
}

function cookieJar(initial = {}) {
  const values = new Map(Object.entries(initial));
  return {
    header() { return [...values].map(([name, value]) => `${name}=${value}`).join('; '); },
    absorb(response) {
      for (const header of response.headers.getSetCookie()) {
        const first = header.split(';', 1)[0];
        const separator = first.indexOf('=');
        if (separator < 0) continue;
        const name = first.slice(0, separator);
        const value = first.slice(separator + 1);
        if (!value) values.delete(name);
        else values.set(name, value);
      }
    }
  };
}

async function request(path, jar, init = {}) {
  const headers = new Headers(init.headers);
  if (jar.header()) headers.set('cookie', jar.header());
  const response = await fetch(new URL(path, baseUrl), { ...init, headers, redirect: 'manual' });
  jar.absorb(response);
  return response;
}

async function postAction(path, jar, fields = {}) {
  return request(path, jar, {
    method: 'POST',
    headers: {
      accept: 'application/json',
      'content-type': 'application/x-www-form-urlencoded',
      'x-sveltekit-action': 'true',
      origin: baseUrl.origin
    },
    body: new URLSearchParams(fields)
  });
}

try {
  insertAccount(ownerId, 'Invite Test Owner', ownerToken);
  insertAccount(memberId, 'Invite Test Member', memberToken);
  sql.prepare('INSERT INTO project (id, name, createdAt) VALUES (?, ?, ?)')
    .run(projectId, 'Invitation HTTP Test', now);
  sql.prepare(`INSERT INTO project_member (projectId, userId, role, createdAt)
    VALUES (?, ?, 'owner', ?)` ).run(projectId, ownerId, now);
  sql.prepare(`INSERT INTO project_subscription (projectId, planId, status, createdAt, updatedAt)
    VALUES (?, 'free', 'simulated', ?, ?)` ).run(projectId, now, now);

  const owner = cookieJar({
    'better-auth.session_token': encodeURIComponent(`${ownerToken}.${createHmac('sha256', devVars.BETTER_AUTH_SECRET).update(ownerToken).digest('base64')}`),
    'q2q-platform-project': projectId
  });
  const member = cookieJar({
    'better-auth.session_token': encodeURIComponent(`${memberToken}.${createHmac('sha256', devVars.BETTER_AUTH_SECRET).update(memberToken).digest('base64')}`)
  });

  const settings = await request('/project/settings', owner);
  assert.equal(settings.status, 200, 'owner can open project management');
  assert.match(await settings.text(), /Invitation HTTP Test/);
  checks.push('owner opens the team project management page');

  const simulate = await postAction('/usage?/simulate', owner, { projectId, planId: 'team' });
  assert.equal(simulate.status, 200);
  const simulateResult = JSON.parse(await simulate.text());
  assert.equal(simulateResult.type, 'success');
  assert.equal(sql.prepare('SELECT planId FROM project_subscription WHERE projectId = ?').get(projectId)?.planId, 'team');
  checks.push('owner can switch a local project to the Team plan through the plan action');

  const create = await postAction('/project/settings?/createInvite', owner);
  assert.equal(create.status, 200, `create invitation action returned ${create.status}`);
  const createBody = await create.text();
  const inviteUrlMatch = createBody.match(/invites(?:\\\/|\/)([A-Za-z0-9_-]{43})/);
  assert.ok(inviteUrlMatch, 'create invite action returns the one-time invite URL');
  const inviteToken = inviteUrlMatch[1];
  const invitePath = `/invites/${inviteToken}`;
  checks.push('owner creates an invitation through the project settings action');

  const anonymous = cookieJar();
  const publicInvite = await request(invitePath, anonymous);
  assert.equal(publicInvite.status, 200);
  assert.match(await publicInvite.text(), /Sign in to accept|登录后接受邀请/);
  assert.ok(anonymous.header().includes('q2q-pending-project-invite='), 'anonymous visit stores a pending invite cookie');
  checks.push('anonymous invite visit records a pending invite for post-login redirect');

  anonymous.absorb(new Response(null, { headers: { 'set-cookie': member.header() } }));
  const afterLogin = await request('/library', anonymous);
  assert.equal(afterLogin.status, 303);
  assert.equal(afterLogin.headers.get('location'), invitePath);
  checks.push('the pending invite redirects a newly signed-in member back to the invite');

  const memberInvite = await request(invitePath, member);
  assert.equal(memberInvite.status, 200);
  assert.match(await memberInvite.text(), /接受邀请|Accept invitation/);
  const accept = await postAction(`${invitePath}?/accept`, member);
  const acceptResult = JSON.parse(await accept.text());
  assert.equal(acceptResult.type, 'redirect');
  assert.equal(acceptResult.status, 303);
  assert.equal(acceptResult.location, '/library');
  assert.equal(sql.prepare('SELECT role FROM project_member WHERE projectId = ? AND userId = ?')
    .get(projectId, memberId)?.role, 'member');
  checks.push('member accepts the invitation and becomes a project member');

  const ownerAfterAccept = await request('/project/settings', owner);
  assert.equal(ownerAfterAccept.status, 200);
  assert.match(await ownerAfterAccept.text(), /Invite Test Member/);
  checks.push('owner sees the newly accepted member');

  const secondCreate = await postAction('/project/settings?/createInvite', owner);
  assert.equal(secondCreate.status, 200);
  const secondCreateBody = await secondCreate.text();
  const secondToken = secondCreateBody.match(/invites(?:\\\/|\/)([A-Za-z0-9_-]{43})/)?.[1];
  assert.ok(secondToken, 'second invite action returns a token for revocation testing');
  const secondHash = createHash('sha256').update(secondToken).digest('hex');
  const secondInvite = sql.prepare('SELECT id FROM project_invitation WHERE tokenHash = ?').get(secondHash);
  assert.ok(secondInvite);
  const revoke = await request('/project/settings?/revokeInvite', owner, {
    method: 'POST',
    headers: {
      accept: 'application/json', 'content-type': 'application/x-www-form-urlencoded',
      'x-sveltekit-action': 'true', origin: baseUrl.origin
    },
    body: new URLSearchParams({ invitationId: secondInvite.id })
  });
  assert.equal(revoke.status, 200);
  const revokedInvite = await request(`/invites/${secondToken}`, member);
  assert.equal(revokedInvite.status, 200);
  assert.match(await revokedInvite.text(), /邀请已撤销|revoked/);
  checks.push('owner revokes a pending invitation and the link reports revoked');

  const removeMember = await request('/project/settings?/removeMember', owner, {
    method: 'POST',
    headers: {
      accept: 'application/json', 'content-type': 'application/x-www-form-urlencoded',
      'x-sveltekit-action': 'true', origin: baseUrl.origin
    },
    body: new URLSearchParams({ userId: memberId })
  });
  assert.equal(removeMember.status, 200);
  assert.equal(sql.prepare('SELECT 1 FROM project_member WHERE projectId = ? AND userId = ?')
    .get(projectId, memberId), undefined);
  const removedAccess = await request(`/project/settings`, member);
  assert.equal(removedAccess.status, 200);
  assert.doesNotMatch(await removedAccess.text(), /Invitation HTTP Test/, 'removed members no longer see the team project');
  checks.push('owner removes the member and project access is revoked');

  console.log(checks.map((name) => `PASS ${name}`).join('\n'));
  console.log(`\n${checks.length} project invitation HTTP checks passed.`);
} finally {
  sql.prepare('DELETE FROM project WHERE id = ?').run(projectId);
  sql.prepare('DELETE FROM "user" WHERE id IN (?, ?)').run(ownerId, memberId);
  sql.close();
}
