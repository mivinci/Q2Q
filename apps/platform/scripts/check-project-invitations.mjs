// In-memory D1-style integration checks for project membership and invitations.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { registerHooks } from 'node:module';
import { DatabaseSync } from 'node:sqlite';

registerHooks({ resolve(specifier, context, next) {
  try { return next(specifier, context); }
  catch (cause) {
    if (context.parentURL?.startsWith(new URL('../src/', import.meta.url).href) && specifier.startsWith('.') && specifier.endsWith('.js')) {
      return next(specifier.slice(0, -3) + '.ts', context);
    }
    throw cause;
  }
} });

const { createProjectInvitation, getProjectInvitationByToken, hashProjectInviteToken,
  invitationStatus, acceptProjectInvitation, revokeProjectInvitation, projectInviteLifetimeMs } =
  await import('../src/lib/server/project-invitations.ts');
const { listProjectMembers, renameProject, removeProjectMember } =
  await import('../src/lib/server/projects.ts');

function fixture() {
  const sql = new DatabaseSync(':memory:');
  for (const migration of ['0001_auth.sql', '0002_projects.sql', '0015_project_subscriptions.sql', '0016_project_invitations.sql', '0017_plan_catalog.sql']) {
    sql.exec(readFileSync(new URL(`../migrations/${migration}`, import.meta.url), 'utf8'));
  }

  let batchTail = Promise.resolve();
  const db = {
    prepare(query) {
      let parameters = [];
      return {
        bind(...values) {
          parameters = values;
          return {
            async first() { return sql.prepare(query).get(...parameters) ?? null; },
            async all() { return { results: sql.prepare(query).all(...parameters) }; },
            async run() {
              if (/\bRETURNING\b/i.test(query)) {
                const results = sql.prepare(query).all(...parameters);
                return { meta: { changes: results.length }, results };
              }
              const result = sql.prepare(query).run(...parameters);
              return { meta: { changes: Number(result.changes) }, results: [] };
            }
          };
        }
      };
    },
    batch(statements) {
      const operation = batchTail.then(async () => {
        sql.exec('BEGIN');
        try {
          const results = [];
          for (const statement of statements) results.push(await statement.run());
          sql.exec('COMMIT');
          return results;
        } catch (cause) {
          sql.exec('ROLLBACK');
          throw cause;
        }
      });
      batchTail = operation.catch(() => {});
      return operation;
    }
  };

  const now = 1_800_000_000_000;
  sql.prepare(`INSERT INTO "user" (id, name, email, createdAt, updatedAt)
    VALUES (?, ?, ?, ?, ?)`).run('owner', 'Owner', 'owner@example.test', now, now);
  for (const [id, name] of [['member', 'Member'], ['other', 'Other'], ['outsider', 'Outsider']]) {
    sql.prepare(`INSERT INTO "user" (id, name, email, createdAt, updatedAt)
      VALUES (?, ?, ?, ?, ?)`).run(id, name, `${id}@example.test`, now, now);
  }
  sql.prepare('INSERT INTO project (id, name, createdAt) VALUES (?, ?, ?)').run('team-project', 'Team Project', now);
  sql.prepare(`INSERT INTO project_member (projectId, userId, role, createdAt)
    VALUES ('team-project', 'owner', 'owner', ?), ('team-project', 'member', 'member', ?)`).run(now, now);
  sql.prepare(`INSERT INTO project_subscription (projectId, planId, status, createdAt, updatedAt)
    VALUES ('team-project', 'team', 'simulated', ?, ?)`).run(now, now);
  sql.prepare('INSERT INTO project (id, name, personalUserId, createdAt) VALUES (?, ?, ?, ?)')
    .run('personal-owner', 'Personal', 'owner', now);
  sql.prepare(`INSERT INTO project_member (projectId, userId, role, createdAt)
    VALUES ('personal-owner', 'owner', 'owner', ?)`).run(now);
  sql.prepare(`INSERT INTO project_subscription (projectId, planId, status, createdAt, updatedAt)
    VALUES ('personal-owner', 'team', 'simulated', ?, ?)`).run(now, now);
  sql.prepare('INSERT INTO project (id, name, createdAt) VALUES (?, ?, ?)').run('free-project', 'Free Project', now);
  sql.prepare(`INSERT INTO project_member (projectId, userId, role, createdAt)
    VALUES ('free-project', 'owner', 'owner', ?)`).run(now);
  sql.prepare(`INSERT INTO project_subscription (projectId, planId, status, createdAt, updatedAt)
    VALUES ('free-project', 'free', 'simulated', ?, ?)`).run(now, now);

  return { db, sql, now, close: () => sql.close() };
}

const checks = [];
async function check(name, run) {
  const f = fixture();
  try { await run(f); checks.push(name); console.log(`PASS ${name}`); }
  finally { f.close(); }
}

await check('invite tokens are random, digest-only and expire after seven days', async ({ db, sql, now }) => {
  const invite = await createProjectInvitation(db, 'team-project', 'owner', now);
  assert.ok(invite);
  assert.match(invite.token, /^[A-Za-z0-9_-]{43}$/);
  assert.equal(invite.expiresAt, now + projectInviteLifetimeMs);
  const stored = sql.prepare('SELECT tokenHash, expiresAt FROM project_invitation WHERE id = ?').get(invite.id);
  assert.equal(stored.tokenHash, await hashProjectInviteToken(invite.token));
  assert.notEqual(stored.tokenHash, invite.token);
  assert.equal(stored.expiresAt, invite.expiresAt);
  const details = await getProjectInvitationByToken(db, stored.tokenHash, 'owner');
  assert.equal(details.projectName, 'Team Project');
  assert.equal(details.inviterName, 'Owner');
  assert.equal(invitationStatus(details, now), 'pending');
});

await check('Personal projects, Free projects and non-owners cannot create invites', async ({ db, now }) => {
  assert.equal(await createProjectInvitation(db, 'personal-owner', 'owner', now), null);
  assert.equal(await createProjectInvitation(db, 'free-project', 'owner', now), null);
  assert.equal(await createProjectInvitation(db, 'team-project', 'member', now), null);
});

await check('there can be at most ten unexpired pending invites', async ({ db, now }) => {
  const invites = [];
  for (let index = 0; index < 10; index++) {
    const invite = await createProjectInvitation(db, 'team-project', 'owner', now);
    assert.ok(invite);
    invites.push(invite);
  }
  assert.equal(await createProjectInvitation(db, 'team-project', 'owner', now), null);
  await db.prepare('UPDATE project_invitation SET expiresAt = ? WHERE id = ?').bind(now, invites[0].id).run();
  assert.ok(await createProjectInvitation(db, 'team-project', 'owner', now));
});

await check('acceptance atomically adds one member and cannot be replayed', async ({ db, sql, now }) => {
  const invite = await createProjectInvitation(db, 'team-project', 'owner', now);
  const hash = await hashProjectInviteToken(invite.token);
  assert.equal(await acceptProjectInvitation(db, hash, 'other', now + 1), 'team-project');
  assert.equal(sql.prepare(`SELECT role FROM project_member WHERE projectId = 'team-project' AND userId = 'other'`).get().role, 'member');
  assert.equal(await acceptProjectInvitation(db, hash, 'outsider', now + 2), null);
  const details = await getProjectInvitationByToken(db, hash, 'other');
  assert.equal(invitationStatus(details, now + 2), 'accepted');
  assert.equal(details.acceptedBy, 'other');
});

await check('parallel acceptance lets only one account consume a link', async ({ db, sql, now }) => {
  const invite = await createProjectInvitation(db, 'team-project', 'owner', now);
  const hash = await hashProjectInviteToken(invite.token);
  const accepted = await Promise.all([
    acceptProjectInvitation(db, hash, 'other', now + 1),
    acceptProjectInvitation(db, hash, 'outsider', now + 1)
  ]);
  assert.equal(accepted.filter(Boolean).length, 1);
  assert.equal(sql.prepare(`SELECT COUNT(*) AS count FROM project_member WHERE projectId = 'team-project' AND userId IN ('other', 'outsider')`).get().count, 1);
});

await check('expired, revoked and downgraded-plan invites cannot be accepted', async ({ db, sql, now }) => {
  const expired = await createProjectInvitation(db, 'team-project', 'owner', now);
  assert.equal(await acceptProjectInvitation(db, await hashProjectInviteToken(expired.token), 'other', expired.expiresAt), null);
  assert.equal(invitationStatus(await getProjectInvitationByToken(db, await hashProjectInviteToken(expired.token), null), expired.expiresAt), 'expired');

  const revoked = await createProjectInvitation(db, 'team-project', 'owner', now);
  assert.equal(await revokeProjectInvitation(db, 'team-project', 'outsider', revoked.id, now + 1), false);
  assert.equal(await revokeProjectInvitation(db, 'team-project', 'owner', revoked.id, now + 1), true);
  assert.equal(await acceptProjectInvitation(db, await hashProjectInviteToken(revoked.token), 'other', now + 2), null);
  assert.equal(invitationStatus(await getProjectInvitationByToken(db, await hashProjectInviteToken(revoked.token), null), now + 2), 'revoked');

  const downgraded = await createProjectInvitation(db, 'team-project', 'owner', now);
  sql.prepare(`UPDATE project_subscription SET planId = 'free' WHERE projectId = 'team-project'`).run();
  assert.equal(await acceptProjectInvitation(db, await hashProjectInviteToken(downgraded.token), 'other', now + 2), null);
});

await check('owners can rename and remove members while non-owners cannot', async ({ db }) => {
  assert.equal(await renameProject(db, 'team-project', 'member', 'Changed'), false);
  assert.equal(await renameProject(db, 'team-project', 'owner', 'Renamed Team'), true);
  assert.equal(await removeProjectMember(db, 'team-project', 'member', 'owner'), false);
  assert.equal(await removeProjectMember(db, 'team-project', 'owner', 'member'), true);
  assert.deepEqual((await listProjectMembers(db, 'team-project')).map(({ userId, role }) => ({ userId, role })), [
    { userId: 'owner', role: 'owner' }
  ]);
});


await check('catalog collaboration edits and expired subscriptions block invite writes', async ({ sql, db }) => {
    const invite = await createProjectInvitation(db, 'team-project', 'owner');
    assert.ok(invite);
    sql.exec("UPDATE plan_catalog SET teamCollaboration = 0 WHERE id = 'team'");
    assert.equal(await createProjectInvitation(db, 'team-project', 'owner'), null);
    assert.equal(await acceptProjectInvitation(db, await hashProjectInviteToken(invite.token), 'other'), null);
    sql.exec("UPDATE plan_catalog SET teamCollaboration = 1 WHERE id = 'team'");
    sql.exec("UPDATE project_subscription SET periodEndAt = 0 WHERE projectId = 'team-project'");
    assert.equal(await createProjectInvitation(db, 'team-project', 'owner'), null);
    assert.equal(await acceptProjectInvitation(db, await hashProjectInviteToken(invite.token), 'other'), null);
});

console.log(`\n${checks.length} project invitation checks passed.`);
