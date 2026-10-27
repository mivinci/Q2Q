import assert from 'node:assert/strict';
import { test } from 'node:test';
import { registerHooks } from 'node:module';
import { MutationError } from '../src/lib/server/mutations.ts';
registerHooks({ resolve(specifier, context, next) {
  try { return next(specifier, context); }
  catch (cause) {
    if (context.parentURL?.startsWith(new URL('../src/', import.meta.url).href) && specifier.startsWith('.') && specifier.endsWith('.js')) return next(specifier.slice(0,-3)+'.ts', context);
    throw cause;
  }
} });
const { mutationAction } = await import('../src/lib/server/mutation-action.ts');
const actor = {id:'verified-subject',email:'operator@example.test'};
const url = new URL('https://admin.test/plans/free');
function request(origin = url.origin) {
  const form = new FormData();
  form.set('requestId','97b39381-ae68-4552-b30d-461955971648');
  form.set('reason','Preserve this edit on retry');
  form.set('password','must-not-be-echoed');
  return new Request(url,{method:'POST',headers:{Origin:origin},body:form});
}
const fields=['requestId','reason'];
test('cross-origin writes and missing verified identity never reach the mutation',async()=>{
  let calls=0;
  const mutate=async()=>{calls++;return {requestId:'id',revision:2,createdAt:0,replayed:false};};
  const cross=await mutationAction(request('https://other.test'),url,actor,fields,mutate);
  const missing=await mutationAction(request(),url,undefined,fields,mutate);
  assert.equal(cross.status,403);assert.equal(missing.status,403);assert.equal(calls,0);
});
test('validation errors preserve only explicitly allowed fields',async()=>{
  const result=await mutationAction(request(),url,actor,fields,async()=>{throw new MutationError('invalidChanges');});
  assert.equal(result.status,400);
  assert.equal(result.data.error,'invalidChanges');
  assert.equal(result.data.values.reason,'Preserve this edit on retry');
  assert.ok(!JSON.stringify(result).includes('must-not-be-echoed'));
});
test('uncertain results keep the original idempotency key and input',async context=>{
  const log=context.mock.method(console,'error',()=>{});
  const result=await mutationAction(request(),url,actor,fields,async()=>{throw Error('provider credential must stay private');});
  assert.equal(result.status,503);assert.equal(result.data.error,'operationUncertain');
  assert.equal(result.data.values.requestId,'97b39381-ae68-4552-b30d-461955971648');
  assert.ok(!JSON.stringify(log.mock.calls).includes('provider credential'));
});
