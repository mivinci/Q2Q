/** Creem test-mode checkout. No production endpoint or live key is supported. */
import { redirect } from '@sveltejs/kit';
import { activeSubscriptionSql } from '@q2q/plans';

export type PaidPlan = 'creator' | 'team';
type Environment = { DB: D1Database; CREEM_MODE?: string; CREEM_API_KEY?: string; CREEM_WEBHOOK_SECRET?: string; CREEM_CREATOR_PRODUCT_ID?: string; CREEM_TEAM_PRODUCT_ID?: string; AUTH_ORIGIN?: string };
type Checkout = { id: string; projectId: string; ownerId: string; planId: PaidPlan; productId: string; status: string; checkoutId: string | null; createdAt: number };
type Subscription = { subscriptionId: string; checkoutId: string; projectId: string; mode: 'test' | 'live'; planId: PaidPlan; productId: string; customerId: string; status: string; providerStatus: string; createdAt: number };
export class BillingError extends Error {}
const billingOrigins = ['https://checkout.creem.io', 'https://www.creem.io', 'https://creem.io'];
const subscriptionStatuses = ['active', 'scheduled_cancel', 'canceled', 'expired', 'paused', 'unpaid', 'past_due'];

export function billingEnabled(env: Environment): boolean {
  return env.CREEM_MODE === 'test' && !!env.CREEM_API_KEY?.startsWith('creem_test_') &&
    !!env.CREEM_WEBHOOK_SECRET && !!env.AUTH_ORIGIN &&
    /^prod_[A-Za-z0-9]+$/.test(env.CREEM_CREATOR_PRODUCT_ID ?? '') &&
    /^prod_[A-Za-z0-9]+$/.test(env.CREEM_TEAM_PRODUCT_ID ?? '') && env.CREEM_CREATOR_PRODUCT_ID !== env.CREEM_TEAM_PRODUCT_ID;
}
export function paidPlan(value: unknown): value is PaidPlan {
  return value === 'creator' || value === 'team';
}
function checkoutUrl(value: unknown): string {
  if (typeof value !== 'string' || value.length > 4096) throw new BillingError('invalid checkout');
  const url = new URL(value);
  if (!billingOrigins.includes(url.origin) ||
      url.port || url.username || url.password || url.hash) throw new BillingError('invalid checkout');
  return url.href;
}

/** SvelteKit 3 requires explicit permission for external redirects. */
export function redirectToBilling(destination: string): never {
  redirect(303, checkoutUrl(destination), {external: billingOrigins});
}

/** Claim before the external write; ambiguity is held for reconciliation. */
export async function createCheckout(env: Environment, projectId: string, ownerId: string, planId: PaidPlan, requestOrigin: string): Promise<string> {
  if (!billingEnabled(env) || new URL(env.AUTH_ORIGIN!).origin !== requestOrigin) throw new BillingError('unavailable');
  const pending = await env.DB.prepare(`SELECT c.checkoutUrl FROM billing_checkout c
    JOIN project_member m ON m.projectId=c.projectId AND m.userId=? AND m.role='owner'
    JOIN user u ON u.id=m.userId AND u.disabledAt IS NULL
    WHERE c.projectId=? AND c.ownerId=? AND c.planId=? AND c.mode='test' AND c.status='open'`)
    .bind(ownerId,projectId,ownerId,planId).first<{checkoutUrl:string|null}>();
  if (pending?.checkoutUrl) return checkoutUrl(pending.checkoutUrl);
  const id = crypto.randomUUID(), now = Date.now(), product = (planId === 'creator' ? env.CREEM_CREATOR_PRODUCT_ID : env.CREEM_TEAM_PRODUCT_ID)!;
  let row: Checkout | null;
  try {
    row = await env.DB.prepare(`INSERT INTO billing_checkout
      (id, projectId, ownerId, mode, planId, productId, status, createdAt, updatedAt)
      SELECT ?, ?, ?, 'test', ?, ?, 'creating', ?, ?
      WHERE EXISTS (SELECT 1 FROM project_member m JOIN user u ON u.id=m.userId
        WHERE m.projectId=? AND m.userId=? AND m.role='owner' AND u.disabledAt IS NULL)
      -- Free placeholders never hold the project; a Creem subscription only
      -- blocks until its current period ends. Manual and simulated subscriptions
      -- (and any effective subscription) must not be overwritten.
      AND NOT EXISTS (SELECT 1 FROM project_subscription s WHERE s.projectId=?
        AND NOT ((s.planId='free' AND s.provider IS NULL)
          OR (s.provider='creem' AND NOT (${activeSubscriptionSql('s', String(now))}))))
      RETURNING *`).bind(id, projectId, ownerId, planId, product, now, now, projectId, ownerId, projectId).first<Checkout>();
  } catch { throw new BillingError('conflict'); }
  if (!row) throw new BillingError('conflict');
  try {
    const response = await fetch('https://test-api.creem.io/v1/checkouts', {
      method: 'POST', headers: {'content-type': 'application/json', 'x-api-key': env.CREEM_API_KEY!},
      body: JSON.stringify({ product_id: product, request_id: id, success_url: new URL('/usage?billing=return', env.AUTH_ORIGIN).href,
        metadata: {q2q_checkout_id: id} }), signal: AbortSignal.timeout(10000)
    });
    if (!response.ok) throw new BillingError('checkout failed');
    const data = await response.json() as {id?:unknown; mode?:unknown; product?:unknown; request_id?:unknown; checkout_url?:unknown; metadata?:unknown};
    if (typeof data.id !== 'string' || !/^ch(?:k)?_[A-Za-z0-9]+$/.test(data.id) || data.mode !== 'test' ||
        externalId(data.product) !== product ||
        (data.request_id != null && data.request_id !== id) || object(data.metadata).q2q_checkout_id !== id) throw new BillingError('unexpected checkout');
    const url = checkoutUrl(data.checkout_url);
    // Recheck the owner and account after the network call. If revoked, do not
    // return a usable payment URL; retain the checkout for reconciliation.
    const saved = await env.DB.prepare(`UPDATE billing_checkout SET status='open', checkoutId=?, checkoutUrl=?, updatedAt=?
      WHERE id=? AND status='creating' AND EXISTS (SELECT 1 FROM project_member m JOIN user u ON u.id=m.userId
        WHERE m.projectId=? AND m.userId=? AND m.role='owner' AND u.disabledAt IS NULL) RETURNING id`)
      .bind(data.id, url, Date.now(), id, projectId, ownerId).first();
    if (!saved) throw new BillingError('owner unavailable');
    return url;
  } catch {
    await env.DB.prepare("UPDATE billing_checkout SET status='unknown', updatedAt=? WHERE id=? AND status='creating'").bind(Date.now(), id).run();
    throw new BillingError('checkout unavailable');
  }
}

/** The customer comes from the project's current Creem subscription, never form data. */
export async function createBillingPortal(env: Environment, projectId: string, ownerId: string): Promise<string> {
  if (!billingEnabled(env)) throw new BillingError('unavailable');
  const customer = await env.DB.prepare(`SELECT bs.customerId FROM billing_subscription bs
    JOIN project_subscription ps ON ps.projectId=bs.projectId AND ps.provider='creem' AND ps.providerSubscriptionId=bs.subscriptionId
    JOIN project_member m ON m.projectId=bs.projectId AND m.userId=? AND m.role='owner'
    JOIN user u ON u.id=m.userId AND u.disabledAt IS NULL
    WHERE bs.projectId=? AND bs.mode='test' AND bs.customerId IS NOT NULL
      AND NOT EXISTS (SELECT 1 FROM billing_subscription other WHERE other.customerId=bs.customerId AND other.mode=bs.mode
        AND NOT EXISTS (SELECT 1 FROM project_member member WHERE member.projectId=other.projectId AND member.userId=? AND member.role='owner'))
      ORDER BY bs.createdAt DESC LIMIT 1`)
    .bind(ownerId,projectId,ownerId).first<{customerId:string}>();
  if (!customer) throw new BillingError('unavailable');
  const response = await fetch('https://test-api.creem.io/v1/customers/billing', {
    method:'POST',headers:{'content-type':'application/json','x-api-key':env.CREEM_API_KEY!},
    body:JSON.stringify({customer_id:customer.customerId}),signal:AbortSignal.timeout(10000)
  });
  if (!response.ok) throw new BillingError('portal unavailable');
  const result = object(await response.json());
  // A revoked owner must not receive a customer portal bearer link.
  const owner = await env.DB.prepare(`SELECT m.userId FROM project_member m JOIN user u ON u.id=m.userId
    WHERE m.projectId=? AND m.userId=? AND m.role='owner' AND u.disabledAt IS NULL`)
    .bind(projectId,ownerId).first();
  if (!owner) throw new BillingError('owner unavailable');
  return checkoutUrl(result.customer_portal_link);
}

type JsonRecord = Record<string, unknown>;
function object(value: unknown): JsonRecord {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new BillingError('invalid event');
  return value as JsonRecord;
}
function externalId(value: unknown): string {
  const id = typeof value === 'string' ? value : object(value).id;
  if (typeof id !== 'string' || !/^[A-Za-z0-9_]{3,128}$/.test(id)) throw new BillingError('invalid identifier');
  return id;
}
function instant(value: unknown): number {
  if (typeof value !== 'string') throw new BillingError('invalid period');
  const parsed = Date.parse(value);
  if (!Number.isSafeInteger(parsed) || parsed < 0) throw new BillingError('invalid period');
  return parsed;
}
function hex(bytes: Uint8Array): string { return Array.from(bytes, value => value.toString(16).padStart(2, '0')).join(''); }

/** Verify raw bytes first. Only the test API can supply subscription state. */
export async function handleCreemWebhook(env: Environment, request: Request): Promise<Response> {
  if (env.CREEM_MODE !== 'test' || !env.CREEM_API_KEY?.startsWith('creem_test_') || !env.CREEM_WEBHOOK_SECRET) return new Response(null, {status: 404});
  const signature = request.headers.get('creem-signature') ?? '';
  if (!/^[0-9a-f]{64}$/i.test(signature) || Number(request.headers.get('content-length') ?? '0') > 1048576) return new Response(null, {status: 401});
  const reader = request.body?.getReader();
  if (!reader) return new Response(null, {status: 400});
  const parts: Uint8Array[] = [];
  let size = 0;
  while (true) {
    const {done, value} = await reader.read();
    if (done) break;
    size += value.length;
    if (size > 1048576) {await reader.cancel(); return new Response(null, {status: 413});}
    parts.push(value);
  }
  const body = new Uint8Array(size);
  let offset = 0;
  for (const part of parts) {body.set(part, offset); offset += part.length;}
  const key = await crypto.subtle.importKey('raw', new TextEncoder().encode(env.CREEM_WEBHOOK_SECRET), {name: 'HMAC', hash: 'SHA-256'}, false, ['verify']);
  const signatureBytes = Uint8Array.from(signature.match(/../g)!, byte => parseInt(byte, 16));
  if (!await crypto.subtle.verify('HMAC', key, signatureBytes, body)) return new Response(null, {status: 401});
  let event: JsonRecord, payload: JsonRecord, id: string, type: string;
  try {
    event = object(JSON.parse(new TextDecoder('utf-8', {fatal: true}).decode(body)));
    id = externalId(event.id);
    if (!id.startsWith('evt_') || typeof event.eventType !== 'string' || event.eventType.length > 100) throw new BillingError('invalid event');
    type = event.eventType; payload = object(event.object);
  } catch { return new Response(null, {status: 400}); }
  const bodyHash = hex(new Uint8Array(await crypto.subtle.digest('SHA-256', body)));
  const existing = await env.DB.prepare('SELECT bodyHash, processedAt FROM billing_webhook_event WHERE id=?').bind(id).first<{bodyHash:string; processedAt:number|null}>();
  if (existing && existing.bodyHash !== bodyHash) return new Response(null, {status: 400});
  if (existing?.processedAt != null) return new Response(null, {status: 200});
  const handled = ['checkout.completed', 'subscription.paid', 'subscription.scheduled_cancel', 'subscription.canceled', 'subscription.expired', 'subscription.paused', 'subscription.past_due', 'subscription.update'];
  // Refunds and disputes require a separate policy; retain retries rather than
  // silently acknowledging an event whose entitlement effect is undecided.
  if (type === 'refund.created' || type === 'dispute.created') return new Response(null, {status: 503});
  if (!handled.includes(type)) {
    await env.DB.prepare('INSERT INTO billing_webhook_event(id,eventType,bodyHash,receivedAt,processedAt) VALUES(?,?,?,?,?) ON CONFLICT(id) DO NOTHING').bind(id,type,bodyHash,Date.now(),Date.now()).run();
    const stored = await env.DB.prepare('SELECT bodyHash FROM billing_webhook_event WHERE id=?').bind(id).first<{bodyHash:string}>();
    return new Response(null, {status: stored?.bodyHash === bodyHash ? 200 : 400});
  }

  // A checkout event owns the checkout row; later subscription events own their
  // subscription row. Neither can rewrite the other project's state.
  let subscriptionId: string, checkout: Checkout | null = null, stored: Subscription | null = null;
  try {
    if (type === 'checkout.completed') {
      const metadata = object(payload.metadata), order = object(payload.order);
      if (payload.mode !== 'test' || order.status !== 'paid') throw new BillingError('not paid');
      const checkoutId = metadata.q2q_checkout_id;
      if (typeof checkoutId !== 'string' || !/^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i.test(checkoutId)) throw new BillingError('invalid checkout');
      checkout = await env.DB.prepare('SELECT * FROM billing_checkout WHERE id=?').bind(checkoutId).first<Checkout>();
      if (!checkout || checkout.status === 'closed' || externalId(payload.product) !== checkout.productId ||
          (checkout.checkoutId && checkout.checkoutId !== externalId(payload.id)) ||
          (payload.request_id != null && payload.request_id !== checkout.id)) throw new BillingError('unknown checkout');
      // The payer must still own an enabled account; otherwise retain the event
      // for reconciliation instead of granting a plan to a revoked owner.
      const owner = await env.DB.prepare(`SELECT 1 AS present FROM project_member m JOIN user u ON u.id=m.userId
        WHERE m.projectId=? AND m.userId=? AND m.role='owner' AND u.disabledAt IS NULL`)
        .bind(checkout.projectId,checkout.ownerId).first();
      if (!owner) throw new BillingError('owner unavailable');
      subscriptionId = externalId(payload.subscription);
      if (!subscriptionId.startsWith('sub_')) throw new BillingError('invalid subscription');
      const linked = await env.DB.prepare('SELECT checkoutId, projectId FROM billing_subscription WHERE subscriptionId=?').bind(subscriptionId).first<{checkoutId:string; projectId:string}>();
      if (linked && (linked.checkoutId !== checkout.id || linked.projectId !== checkout.projectId)) throw new BillingError('subscription conflict');
    } else {
      subscriptionId = externalId(payload.id);
      if (!subscriptionId.startsWith('sub_')) throw new BillingError('invalid subscription');
      stored = await env.DB.prepare('SELECT * FROM billing_subscription WHERE subscriptionId=?').bind(subscriptionId).first<Subscription>();
      if (!stored || stored.mode !== 'test') throw new BillingError('unknown subscription');
    }
    if (!billingEnabled(env)) throw new BillingError('billing disabled');
  } catch { return new Response(null, {status: 503}); }

  const lock = crypto.randomUUID(), now = Date.now();
  const claimed = type === 'checkout.completed'
    ? await env.DB.prepare(`UPDATE billing_checkout SET reconcileToken=?, reconcileUntil=?
        WHERE id=? AND status IN ('creating','open','unknown','paid') AND (reconcileUntil IS NULL OR reconcileUntil<?) RETURNING id`)
        .bind(lock, now + 30000, checkout!.id, now).first()
    : await env.DB.prepare(`UPDATE billing_subscription SET reconcileToken=?, reconcileUntil=?
        WHERE subscriptionId=? AND (reconcileUntil IS NULL OR reconcileUntil<?) RETURNING subscriptionId`)
        .bind(lock, now + 30000, subscriptionId, now).first();
  if (!claimed) return new Response(null, {status: 503});
  const release = type === 'checkout.completed'
    ? () => env.DB.prepare('UPDATE billing_checkout SET reconcileToken=NULL,reconcileUntil=NULL WHERE id=? AND reconcileToken=?').bind(checkout!.id, lock).run()
    : () => env.DB.prepare('UPDATE billing_subscription SET reconcileToken=NULL,reconcileUntil=NULL WHERE subscriptionId=? AND reconcileToken=?').bind(subscriptionId, lock).run();
  try {
    const response = await fetch(`https://test-api.creem.io/v1/subscriptions?subscription_id=${encodeURIComponent(subscriptionId)}`, {
      headers: {'x-api-key': env.CREEM_API_KEY!}, signal: AbortSignal.timeout(10000)
    });
    if (!response.ok) throw new BillingError('subscription unavailable');
    const subscription = object(await response.json());
    const productId = externalId(subscription.product), customerId = externalId(subscription.customer);
    const start = instant(subscription.current_period_start_date), end = instant(subscription.current_period_end_date);
    const product = checkout ? checkout.productId : stored!.productId;
    if (subscription.id !== subscriptionId || subscription.mode !== 'test' || productId !== product || end <= start ||
        (stored && stored.customerId !== customerId)) throw new BillingError('subscription mismatch');
    const status = subscription.status;
    if (typeof status !== 'string' || !subscriptionStatuses.includes(status)) throw new BillingError('unknown status');
    const active = (status === 'active' || status === 'scheduled_cancel') && end > Date.now();
    const mapped = active ? 'active' : status === 'expired' ? 'expired' : 'cancelled';
    const cancelAtPeriodEnd = status === 'scheduled_cancel' ? 1 : 0;
    const marked = Date.now();
    const projectId = checkout ? checkout.projectId : stored!.projectId;
    const planId = checkout ? checkout.planId : stored!.planId;
    const checkoutId = checkout ? checkout.id : stored!.checkoutId;
    const subCreatedAt = checkout ? checkout.createdAt : stored!.createdAt;
    const guards = "EXISTS (SELECT 1 FROM billing_webhook_event e WHERE e.id=? AND e.bodyHash=? AND e.processedAt IS NULL)";
    // A newer subscription always wins: only the newest row may drive
    // project_subscription, so a late event from a replaced subscription cannot
    // revoke the current one.
    const newest = `NOT EXISTS (SELECT 1 FROM billing_subscription newer WHERE newer.projectId=?
      AND (newer.createdAt > ? OR (newer.createdAt = ? AND newer.subscriptionId > ?)))`;
    const statements = [
      env.DB.prepare(`INSERT INTO billing_webhook_event(id,eventType,bodyHash,checkoutId,subscriptionId,receivedAt)
        VALUES(?,?,?,?,?,?) ON CONFLICT(id) DO NOTHING`).bind(id,type,bodyHash,checkoutId,subscriptionId,marked)
    ];
    if (type === 'checkout.completed') {
      statements.push(env.DB.prepare(`INSERT INTO billing_subscription
          (subscriptionId, checkoutId, projectId, mode, planId, productId, customerId, status, providerStatus,
           periodStartAt, periodEndAt, cancelAtPeriodEnd, createdAt, updatedAt)
        SELECT ?,?,?,'test',?,?,?,?,?,?,?,?,?,? WHERE ${guards}
        ON CONFLICT(subscriptionId) DO UPDATE SET status=excluded.status, providerStatus=excluded.providerStatus,
          periodStartAt=excluded.periodStartAt, periodEndAt=excluded.periodEndAt,
          cancelAtPeriodEnd=excluded.cancelAtPeriodEnd, updatedAt=excluded.updatedAt
        WHERE billing_subscription.checkoutId=excluded.checkoutId AND billing_subscription.projectId=excluded.projectId`)
        .bind(subscriptionId,checkoutId,projectId,planId,productId,customerId,mapped,status,start,end,cancelAtPeriodEnd,subCreatedAt,marked,id,bodyHash));
      statements.push(env.DB.prepare(`UPDATE billing_checkout SET status='paid', checkoutId=COALESCE(checkoutId, ?), updatedAt=?
        WHERE id=? AND reconcileToken=? AND reconcileUntil>? AND ${guards}`)
        .bind(externalId(payload.id),marked,checkoutId,lock,marked,id,bodyHash));
    } else {
      statements.push(env.DB.prepare(`UPDATE billing_subscription SET status=?, providerStatus=?, periodStartAt=?,
          periodEndAt=?, cancelAtPeriodEnd=?, updatedAt=? WHERE subscriptionId=? AND ${guards}`)
        .bind(mapped,status,start,end,cancelAtPeriodEnd,marked,subscriptionId,id,bodyHash));
    }
    statements.push(env.DB.prepare(`INSERT INTO project_subscription
        (projectId, planId, status, provider, providerSubscriptionId, periodStartAt, periodEndAt, cancelAtPeriodEnd, createdAt, updatedAt)
      SELECT ?, ?, ?, 'creem', ?, ?, ?, ?, ?, ?
      WHERE NOT EXISTS (SELECT 1 FROM project_subscription WHERE projectId=?)
        AND ${newest} AND ${guards}`)
      .bind(projectId,planId,mapped,subscriptionId,start,end,cancelAtPeriodEnd,marked,marked,projectId,projectId,subCreatedAt,subCreatedAt,subscriptionId,id,bodyHash));
    statements.push(env.DB.prepare(`UPDATE project_subscription
        SET planId=?, status=?, provider='creem', providerSubscriptionId=?, periodStartAt=?, periodEndAt=?,
          cancelAtPeriodEnd=?, updatedAt=?
      WHERE projectId=? AND ${newest}
        AND ((provider='creem' AND providerSubscriptionId=?)
          OR (planId='free' AND provider IS NULL)
          OR (provider='creem' AND NOT (${activeSubscriptionSql('project_subscription', String(marked))})))
        AND ${guards}`)
      .bind(planId,mapped,subscriptionId,start,end,cancelAtPeriodEnd,marked,projectId,projectId,subCreatedAt,subCreatedAt,subscriptionId,subscriptionId,id,bodyHash));
    const locked = type === 'checkout.completed'
      ? `EXISTS (SELECT 1 FROM billing_checkout WHERE id=? AND reconcileToken=? AND reconcileUntil>?)`
      : `EXISTS (SELECT 1 FROM billing_subscription WHERE subscriptionId=? AND reconcileToken=? AND reconcileUntil>?)`;
    statements.push(env.DB.prepare(`UPDATE billing_webhook_event SET processedAt=? WHERE id=? AND bodyHash=? AND processedAt IS NULL AND ${locked}`)
      .bind(marked,id,bodyHash,type === 'checkout.completed' ? checkoutId : subscriptionId,lock,marked));
    await env.DB.batch(statements);
    const processed = await env.DB.prepare('SELECT bodyHash,processedAt FROM billing_webhook_event WHERE id=?').bind(id).first<{bodyHash:string; processedAt:number|null}>();
    if (!processed || processed.bodyHash !== bodyHash || processed.processedAt === null) throw new BillingError('not applied');
    return new Response(null, {status: 200});
  } catch { return new Response(null, {status: 503}); }
  finally { await release(); }
}
