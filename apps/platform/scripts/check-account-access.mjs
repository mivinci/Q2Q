// Verify the installed Better Auth configuration and session hook without OAuth,
// browser interaction, provider requests or an application server.
import assert from 'node:assert/strict';
import { DatabaseSync } from 'node:sqlite';
import { readFileSync, readdirSync } from 'node:fs';
import { registerHooks } from 'node:module';
registerHooks({ resolve(specifier, context, next) {
  if (specifier === '$app/server') return {url:'data:text/javascript,export const getRequestEvent=()=>{throw new Error("No request event in this check")}',shortCircuit:true};
  try { return next(specifier, context); }
  catch (cause) {
    if (context.parentURL?.startsWith(new URL('../src/', import.meta.url).href) && specifier.startsWith('.') && specifier.endsWith('.js')) return next(specifier.slice(0,-3)+'.ts',context);
    throw cause;
  }
} });
const { createAuth } = await import('../src/lib/server/auth.ts');
const { authorizeAccountSession } = await import('@q2q/accounts');
const sql=new DatabaseSync(':memory:');
const migrations=new URL('../migrations/',import.meta.url);
for(const name of readdirSync(migrations).sort()) if(name.endsWith('.sql')) sql.exec(readFileSync(new URL(name,migrations),'utf8'));
sql.exec("INSERT INTO user (id,name,email,createdAt,updatedAt,authVersion) VALUES ('user','User','user@example.test',0,0,7)");
const db={
 async exec(query){sql.exec(query);},
 async batch(statements){return Promise.all(statements.map(statement=>statement.all()));},
 prepare(query){let args=[];return {
  bind(...values){args=values;return this;},
  async first(){return sql.prepare(query).get(...args)??null;},
  async all(){const results=sql.prepare(query).all(...args);return {results,success:true,meta:{changes:Number(sql.prepare('SELECT changes() AS count').get().count)}};}
 };}
};
const auth=createAuth({DB:db,AUTH_ORIGIN:'https://platform.example.test',BETTER_AUTH_SECRET:'test-only-not-a-real-secret-1234567890',GITHUB_CLIENT_ID:'test-only',GITHUB_CLIENT_SECRET:'test-only'});
assert.ok(auth);
const context=await auth.$context;
assert.equal(auth.options.session.cookieCache.enabled,false);
assert.equal(auth.options.session.additionalFields.authVersion.input,false);
assert.equal(auth.options.session.additionalFields.authVersion.returned,false);
const before=auth.options.databaseHooks.session.create.before;
const original={userId:'user',authVersion:999};
assert.equal((await before(original,null)).data.authVersion,7);
assert.equal(original.authVersion,999);
const session=await context.internalAdapter.createSession('user');
assert.ok(session?.id);
const stored=sql.prepare('SELECT authVersion,expiresAt FROM session WHERE id=?').get(session.id);
assert.equal(stored.authVersion,7);
const expiry=typeof stored.expiresAt==='number'?stored.expiresAt:Date.parse(stored.expiresAt);
assert.ok(Number.isSafeInteger(expiry));
assert.equal((await authorizeAccountSession(db,session.id,'user',expiry-1))?.id,'user');
assert.equal(await authorizeAccountSession(db,session.id,'user',expiry),null);
sql.prepare("UPDATE user SET disabledAt=?, disabledReason='Test disable', authVersion=8 WHERE id='user'").run(Date.now());
assert.equal(await authorizeAccountSession(db,session.id,'user'),null);
await assert.rejects(before(original,null),cause=>cause.status==='FORBIDDEN' && cause.statusCode===403);
await assert.rejects(context.internalAdapter.createSession('user'),cause=>cause.statusCode===403);
sql.exec("UPDATE user SET disabledAt=NULL, disabledReason=NULL, authVersion=9 WHERE id='user'");
const restored=await context.internalAdapter.createSession('user');
assert.equal(sql.prepare('SELECT authVersion FROM session WHERE id=?').get(restored.id).authVersion,9);
assert.equal(await authorizeAccountSession(db,session.id,'user'),null);
assert.equal((await authorizeAccountSession(db,restored.id,'user'))?.id,'user');
console.log('PASS installed Better Auth stamps the current server version, hides it from clients and rejects disabled accounts');

process.once('beforeExit',()=>sql.close());
