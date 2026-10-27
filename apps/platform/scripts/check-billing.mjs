import assert from 'node:assert/strict';
import test from 'node:test';
import { DatabaseSync } from 'node:sqlite';
import { readFileSync } from 'node:fs';
import { registerHooks } from 'node:module';
import { billingEnabled, createCheckout, createBillingPortal, redirectToBilling, handleCreemWebhook } from '../src/lib/server/creem.ts';
import { isRedirect } from '@sveltejs/kit';

function fixture() {
  const sql = new DatabaseSync(':memory:');
  sql.exec(`PRAGMA foreign_keys=ON;
    CREATE TABLE user(id TEXT PRIMARY KEY, disabledAt INTEGER);
    CREATE TABLE project(id TEXT PRIMARY KEY,name TEXT DEFAULT 'Personal',personalUserId TEXT);
    CREATE TABLE project_member(projectId TEXT, userId TEXT, role TEXT);
    CREATE TABLE project_subscription(projectId TEXT PRIMARY KEY,planId TEXT,status TEXT,provider TEXT,providerSubscriptionId TEXT,
      periodStartAt INTEGER,periodEndAt INTEGER,cancelAtPeriodEnd INTEGER,createdAt INTEGER,updatedAt INTEGER,revision INTEGER DEFAULT 1);
    INSERT INTO user VALUES('owner',NULL),('member',NULL);
    INSERT INTO project(id) VALUES('p'),('other');
    INSERT INTO project_member VALUES('p','owner','owner'),('p','member','member'),('other','owner','owner');`);
  sql.exec(readFileSync(new URL('../migrations/0021_billing.sql', import.meta.url), 'utf8'));
  const db = {
    prepare(query) { let params=[]; return {bind(...args) {params=args; return this;},
      async first() {return sql.prepare(query).get(...params) ?? null;},
      async run() {return sql.prepare(query).run(...params);},
      runSync() {return sql.prepare(query).run(...params);} }; },
    async batch(statements) {sql.exec('BEGIN'); try {const results=statements.map(s=>s.runSync()); sql.exec('COMMIT'); return results;}
      catch (error) {sql.exec('ROLLBACK'); throw error;}}
  };
  const env={DB:db, CREEM_MODE:'test', CREEM_API_KEY:'creem_test_example', CREEM_WEBHOOK_SECRET:'fixture-key',
    AUTH_ORIGIN:'https://platform.example',
    CREEM_CREATOR_PRODUCT_ID:'prod_creator', CREEM_TEAM_PRODUCT_ID:'prod_team'};
  return {sql,db,env};
}
async function signed(env, event, signature) {
  const body=JSON.stringify(event), key=await crypto.subtle.importKey('raw',new TextEncoder().encode(env.CREEM_WEBHOOK_SECRET),{name:'HMAC',hash:'SHA-256'},false,['sign']);
  const digest=new Uint8Array(await crypto.subtle.sign('HMAC',key,new TextEncoder().encode(body)));
  const hex=Array.from(digest,b=>b.toString(16).padStart(2,'0')).join('');
  return new Request('https://platform.example/api/creem/webhook',{method:'POST',headers:{'creem-signature':signature ?? hex},body});
}
function mockProvider() {
  let checkout, status='active', calls=0;
  return { get checkout() {return checkout;}, get calls() {return calls;}, set status(value) {status=value;},
    async fetch(url, options) {
      calls++;
      if (String(url).endsWith('/v1/checkouts')) {
        checkout=JSON.parse(options.body);
        // The sandbox returns expanded product and metadata, but omits request_id.
        return Response.json({id:'ch_123',mode:'test',product:{id:checkout.product_id},metadata:checkout.metadata,checkout_url:'https://checkout.creem.io/ch_123'});
      }
      if (String(url).endsWith('/v1/customers/billing')) {
        assert.equal(JSON.parse(options.body).customer_id,'cust_123');
        return Response.json({customer_portal_link:'https://creem.io/portal/cust_123?token=fixture'});
      }
      return Response.json({id:'sub_123',mode:'test',product:checkout.product_id,customer:'cust_123',status,
        current_period_start_date:'2026-01-01T00:00:00.000Z',current_period_end_date:'2099-01-01T00:00:00.000Z'});
    }};
}

test('checkout and portal destinations produce real SvelteKit 303 redirects', () => {
  for (const destination of ['https://creem.io/test/payment/ch_example','https://checkout.creem.io/ch_example','https://www.creem.io/portal/cust_example?token=fixture']) {
    assert.throws(()=>redirectToBilling(destination),error=>isRedirect(error) && error.status===303 && error.location===destination);
  }
  for (const destination of ['https://creem.io.attacker.example/','http://creem.io/','https://user:password@creem.io/','javascript:alert(1)']) {
    assert.throws(()=>redirectToBilling(destination),error=>!isRedirect(error));
  }
});

test('real checkout form action redirects both a new purchase and a resumed checkout', async t => {
  const {sql,env}=fixture(),provider=mockProvider(),original=globalThis.fetch;
  const hooks=registerHooks({resolve(specifier,context,next) {
    if (specifier==='cloudflare:workers') return {url:'data:text/javascript,export const env={};',shortCircuit:true};
    if (specifier.startsWith('#lib/')) return next(new URL('../src/lib/'+specifier.slice(5).replace(/\.js$/,'.ts'),import.meta.url).href,context);
    try {return next(specifier,context);}
    catch (error) {
      if (context.parentURL?.startsWith(new URL('../src/',import.meta.url).href) && specifier.startsWith('.') && specifier.endsWith('.js')) return next(specifier.slice(0,-3)+'.ts',context);
      throw error;
    }
  }});
  t.after(()=>{hooks.deregister();globalThis.fetch=original;sql.close();});
  Object.assign((await import('cloudflare:workers')).env,env);
  const {actions}=await import('../src/routes/usage/+page.server.ts');
  globalThis.fetch=provider.fetch;
  const event=()=>({locals:{user:{id:'owner'}},url:new URL('https://platform.example/usage?/checkout'),
    request:new Request('https://platform.example/usage?/checkout',{method:'POST',headers:{origin:'https://platform.example'},body:new URLSearchParams({projectId:'p',planId:'creator'})})});
  for (let attempt=0;attempt<2;attempt++) {
    await assert.rejects(actions.checkout(event()),error=>isRedirect(error) && error.status===303 && error.location==='https://checkout.creem.io/ch_123');
  }
  assert.equal(provider.calls,1);
  sql.exec(`UPDATE billing_checkout SET status='paid';
    INSERT INTO billing_subscription(subscriptionId,checkoutId,projectId,mode,planId,productId,customerId,status,providerStatus,
      periodStartAt,periodEndAt,cancelAtPeriodEnd,createdAt,updatedAt)
    SELECT 'sub_123',id,'p','test','creator','prod_creator','cust_123','active','active',0,9999999999999,0,createdAt,createdAt FROM billing_checkout;
    INSERT INTO project_subscription(projectId,planId,status,provider,providerSubscriptionId) VALUES('p','creator','active','creem','sub_123');`);
  await assert.rejects(actions.portal(event()),error=>isRedirect(error) && error.status===303 && error.location==='https://creem.io/portal/cust_123?token=fixture');
});

test('checkout claims one project, verifies owner and protects manual subscriptions', async t => {
  const {sql,env}=fixture(), provider=mockProvider(), original=globalThis.fetch;
  globalThis.fetch=provider.fetch; t.after(()=>{globalThis.fetch=original; sql.close();});
  assert.equal(billingEnabled(env),true);
  assert.equal(billingEnabled({...env,CREEM_MODE:'live'}),false);
  await assert.rejects(createCheckout(env,'p','member','creator','https://platform.example'));
  assert.equal(await createCheckout(env,'p','owner','creator','https://platform.example'),'https://checkout.creem.io/ch_123');
  assert.equal(await createCheckout(env,'p','owner','creator','https://platform.example'),'https://checkout.creem.io/ch_123');
  await assert.rejects(createCheckout(env,'p','owner','team','https://platform.example'));
  assert.equal(provider.calls,1);
  assert.throws(()=>sql.prepare("INSERT INTO project_subscription(projectId,planId,status,provider) VALUES('p','team','active','manual')").run(),/billing subscription conflict/);
});

test('ambiguous provider failure remains locked instead of creating another checkout', async t => {
  const {sql,env}=fixture(), original=globalThis.fetch; let calls=0;
  globalThis.fetch=async()=>{calls++;throw new Error('timeout');}; t.after(()=>{globalThis.fetch=original;sql.close();});
  await assert.rejects(createCheckout(env,'p','owner','creator','https://platform.example'));
  assert.equal(sql.prepare('SELECT status FROM billing_checkout').get().status,'unknown');
  await assert.rejects(createCheckout(env,'p','owner','creator','https://platform.example'));
  assert.equal(calls,1);
});

test('signed paid event verifies authoritative subscription, grants once and preserves cancellation', async t => {
  const {sql,env}=fixture(), provider=mockProvider(), original=globalThis.fetch;
  sql.exec("INSERT INTO project_subscription(projectId,planId,status) VALUES('p','free','active')");
  globalThis.fetch=provider.fetch; t.after(()=>{globalThis.fetch=original;sql.close();});
  await createCheckout(env,'p','owner','creator','https://platform.example');
  const event={id:'evt_123',eventType:'checkout.completed',object:{id:'ch_123',mode:'test',
    product:'prod_creator',metadata:{q2q_checkout_id:provider.checkout.request_id},order:{status:'paid'},subscription:'sub_123'}};
  assert.equal((await handleCreemWebhook(env,await signed(env,event,'00'.repeat(32)))).status,401);
  assert.equal((await handleCreemWebhook(env,await signed(env,{...event,object:{...event.object,mode:'live'}}))).status,503);
  assert.equal((await handleCreemWebhook(env,await signed(env,{...event,object:{...event.object,product:'prod_other'}}))).status,503);
  assert.equal(sql.prepare('SELECT planId FROM project_subscription').get().planId,'free');
  assert.equal((await handleCreemWebhook(env,await signed(env,event))).status,200);
  assert.equal((await handleCreemWebhook(env,await signed(env,event))).status,200);
  assert.equal((await handleCreemWebhook(env,await signed(env,{...event, eventType:'subscription.paid'}))).status,400);
  let row=sql.prepare('SELECT * FROM project_subscription').get();
  assert.equal(row.planId,'creator');assert.equal(row.provider,'creem');assert.equal(row.status,'active');
  let history=sql.prepare('SELECT * FROM billing_subscription').get();
  assert.equal(history.status,'active');assert.equal(history.providerStatus,'active');assert.equal(history.customerId,'cust_123');
  assert.equal(await createBillingPortal(env,'p','owner'),'https://creem.io/portal/cust_123?token=fixture');
  await assert.rejects(createBillingPortal(env,'p','member'));
  await assert.rejects(createBillingPortal(env,'other','owner'));
  assert.throws(()=>sql.prepare("UPDATE project_subscription SET provider='manual' WHERE projectId='p'").run(),/billing subscription conflict/);
  provider.status='scheduled_cancel';
  const cancel={id:'evt_124',eventType:'subscription.scheduled_cancel',object:{id:'sub_123'}};
  assert.equal((await handleCreemWebhook(env,await signed(env,cancel))).status,200);
  row=sql.prepare('SELECT * FROM project_subscription').get();assert.equal(row.status,'active');assert.equal(row.cancelAtPeriodEnd,1);
  history=sql.prepare('SELECT * FROM billing_subscription').get();assert.equal(history.cancelAtPeriodEnd,1);assert.equal(history.providerStatus,'scheduled_cancel');
  assert.equal(sql.prepare('SELECT count(*) n FROM billing_webhook_event').get().n,2);
});

test('revoked owner cannot complete checkout or apply a paid event', async t => {
  const {sql,env}=fixture(),provider=mockProvider(),original=globalThis.fetch;
  globalThis.fetch=async (...args) => {
    const response=await provider.fetch(...args);
    sql.exec("DELETE FROM project_member WHERE userId='owner' AND projectId='p'");
    return response;
  };
  t.after(()=>{globalThis.fetch=original;sql.close();});
  await assert.rejects(createCheckout(env,'p','owner','creator','https://platform.example'));
  assert.equal(sql.prepare('SELECT status FROM billing_checkout').get().status,'unknown');
  globalThis.fetch=provider.fetch;
  const event={id:'evt_revoked',eventType:'checkout.completed',object:{id:'ch_123',mode:'test',request_id:provider.checkout.request_id,
    product:'prod_creator',metadata:{q2q_checkout_id:provider.checkout.request_id},order:{status:'paid'},subscription:'sub_123'}};
  assert.equal((await handleCreemWebhook(env,await signed(env,event))).status,503);
  assert.equal(sql.prepare('SELECT count(*) n FROM project_subscription').get().n,0);
  assert.equal(sql.prepare('SELECT processedAt FROM billing_webhook_event').get()?.processedAt ?? null,null);
});

function repurchaseProvider() {
  const subscriptions=new Map(); let checkouts=0;
  return {
    add(id,product,customer,status='active',end='2099-01-01T00:00:00.000Z') {
      subscriptions.set(id,{product,customer,status,start:'2026-01-01T00:00:00.000Z',end});
    },
    async fetch(url, options) {
      const target=String(url);
      if (target.endsWith('/v1/checkouts')) {
        const body=JSON.parse(options.body), id=`ch_${++checkouts}`;
        return Response.json({id,mode:'test',product:{id:body.product_id},metadata:body.metadata,checkout_url:`https://checkout.creem.io/${id}`});
      }
      if (target.endsWith('/v1/customers/billing')) {
        const {customer_id}=JSON.parse(options.body);
        return Response.json({customer_portal_link:`https://creem.io/portal/${customer_id}?token=fixture`});
      }
      const id=new URL(target).searchParams.get('subscription_id'), subscription=subscriptions.get(id);
      if (!subscription) return new Response(null,{status:404});
      return Response.json({id,mode:'test',product:subscription.product,customer:subscription.customer,status:subscription.status,
        current_period_start_date:subscription.start,current_period_end_date:subscription.end});
    }
  };
}

test('a project can buy again after its Creem subscription ends', async t => {
  const {sql,env}=fixture(), provider=repurchaseProvider(), original=globalThis.fetch;
  globalThis.fetch=provider.fetch; t.after(()=>{globalThis.fetch=original;sql.close();});
  provider.add('sub_old','prod_creator','cust_old','canceled');
  sql.exec(`INSERT INTO billing_checkout(id,projectId,ownerId,mode,planId,productId,status,checkoutId,createdAt,updatedAt)
      VALUES('checkout-old','p','owner','test','creator','prod_creator','paid','ch_old',1,1);
    INSERT INTO billing_subscription(subscriptionId,checkoutId,projectId,mode,planId,productId,customerId,status,providerStatus,
      periodStartAt,periodEndAt,cancelAtPeriodEnd,createdAt,updatedAt)
      VALUES('sub_old','checkout-old','p','test','creator','prod_creator','cust_old','cancelled','canceled',0,1,0,1,1);
    INSERT INTO project_subscription(projectId,planId,status,provider,providerSubscriptionId,periodStartAt,periodEndAt,cancelAtPeriodEnd,createdAt,updatedAt)
      VALUES('p','creator','cancelled','creem','sub_old',0,1,0,1,1);`);

  assert.equal(await createCheckout(env,'p','owner','team','https://platform.example'),'https://checkout.creem.io/ch_1');
  const checkout=sql.prepare("SELECT * FROM billing_checkout WHERE status='open'").get();
  assert.equal(checkout.planId,'team');
  assert.equal(sql.prepare('SELECT count(*) n FROM billing_checkout').get().n,2);

  provider.add('sub_new','prod_team','cust_new');
  const paid={id:'evt_new',eventType:'checkout.completed',object:{id:'ch_1',mode:'test',product:'prod_team',
    metadata:{q2q_checkout_id:checkout.id},order:{status:'paid'},subscription:'sub_new'}};
  assert.equal((await handleCreemWebhook(env,await signed(env,paid))).status,200);
  let subscription=sql.prepare("SELECT * FROM project_subscription WHERE projectId='p'").get();
  assert.equal(subscription.planId,'team');assert.equal(subscription.providerSubscriptionId,'sub_new');assert.equal(subscription.status,'active');
  assert.equal(sql.prepare("SELECT status FROM billing_checkout WHERE id='checkout-old'").get().status,'paid');
  assert.equal(sql.prepare("SELECT status FROM billing_subscription WHERE subscriptionId='sub_old'").get().status,'cancelled');
  assert.equal(await createBillingPortal(env,'p','owner'),'https://creem.io/portal/cust_new?token=fixture');

  // A late event for the replaced subscription must not revoke the new one, and
  // its delivery still counts as processed instead of retrying forever.
  const late={id:'evt_late',eventType:'subscription.canceled',object:{id:'sub_old'}};
  assert.equal((await handleCreemWebhook(env,await signed(env,late))).status,200);
  subscription=sql.prepare("SELECT * FROM project_subscription WHERE projectId='p'").get();
  assert.equal(subscription.providerSubscriptionId,'sub_new');assert.equal(subscription.status,'active');
  assert.notEqual(sql.prepare("SELECT processedAt FROM billing_webhook_event WHERE id='evt_late'").get().processedAt,null);

  // While the replacement is effective, another checkout is refused.
  await assert.rejects(createCheckout(env,'p','owner','creator','https://platform.example'));
  assert.equal(sql.prepare('SELECT count(*) n FROM billing_checkout').get().n,2);

  // Once it ends, a third purchase is allowed.
  sql.exec("UPDATE project_subscription SET periodEndAt=1 WHERE projectId='p'");
  assert.equal(await createCheckout(env,'p','owner','creator','https://platform.example'),'https://checkout.creem.io/ch_2');
});

test('manual and simulated subscriptions still block a new checkout', async t => {
  const {sql,env}=fixture(), provider=repurchaseProvider(), original=globalThis.fetch;
  globalThis.fetch=provider.fetch; t.after(()=>{globalThis.fetch=original;sql.close();});
  sql.exec(`INSERT INTO project_subscription(projectId,planId,status,provider,periodStartAt,periodEndAt,cancelAtPeriodEnd)
      VALUES('p','creator','active','manual',0,9999999999999,0),
            ('other','team','simulated','local',NULL,NULL,0);`);
  await assert.rejects(createCheckout(env,'p','owner','creator','https://platform.example'));
  await assert.rejects(createCheckout(env,'other','owner','creator','https://platform.example'));
  assert.equal(sql.prepare('SELECT count(*) n FROM billing_checkout').get().n,0);
});
