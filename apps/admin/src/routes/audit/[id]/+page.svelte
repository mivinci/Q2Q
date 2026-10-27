<script lang="ts">
  import { enhance } from '$app/forms';
  import MutationNotice from '#lib/components/mutation-notice.svelte';
  import { page } from '$app/state';
  import { messages } from '#lib/messages.js';
  import { bytes, date, label } from '#lib/format.js';
  import { Button } from '@q2q/ui/button';
  import ResponsiveMetadata from '@q2q/ui/responsive-metadata';
  import type { PageProps } from './$types';
  let { data,form }: PageProps = $props();
  const text = $derived(messages[page.data.locale]);
  const before = $derived(JSON.parse(data.record.beforeJson) as Record<string, unknown> | null);
  const after = $derived((JSON.parse(data.record.afterJson) ?? {}) as Record<string, unknown>);
  const keys = $derived([...new Set([...Object.keys(before ?? {}), ...Object.keys(after)])].filter(key => key !== 'planSnapshot'));
  const items = $derived([
    { label: text.requestId, value: data.record.requestId },
    ...(data.record.action==='punch.policy'?[{label:text.requestedPolicy,value:data.record.requestedEnabled?text.discoverable:text.discoveryStopped}]:[]),
    { label: text.operationStatus, value: label(data.record.status,page.data.locale) },
    { label: text.action, value: label(data.record.action, page.data.locale) },
    { label: text.targetId, value: data.record.targetId },
    { label: text.actorEmail, value: data.record.actorEmail },
    { label: text.actorId, value: data.record.actorId },
    { label: text.createdAt, value: date(data.record.createdAt, page.data.locale) }
  ]);
  function display(value: unknown, key: string) {
    if (value === null || value === undefined) return '—';
    if (key === 'enabled') return value ? text.discoverable : text.discoveryStopped;
    if (typeof value === 'boolean') return value ? text.yes : text.no;
    if (key === 'disabledReason') return String(value);
    if (key.endsWith('Bytes')) return bytes(Number(value));
    if (key === 'periodStartAt' || key === 'periodEndAt' || key === 'disabledAt') return date(Number(value), page.data.locale);
    if (['platformStorage', 'platformNetwork', 'customTrack', 'higherLimits', 'teamCollaboration', 'cancelAtPeriodEnd'].includes(key)) return value ? text.yes : text.no;
    return label(String(value), page.data.locale);
  }
</script>
<svelte:head><title>{text.auditDetails} · Q2Q Admin</title></svelte:head>
<div class="mx-auto w-full max-w-5xl px-4 pt-5 pb-7 sm:px-7">
  <h1 class="text-2xl font-medium tracking-tight">{text.auditDetails}</h1>
  {#if data.record.status!=='applied'}<p role="status" class="mt-5 rounded-xl border bg-card p-4 text-sm">{data.record.status==='pending'?text.registryPending:text.registryRejected}</p>{/if}
  {#if data.record.status!=='applied'||form?.saved}<MutationNotice {form} />{/if}
  {#if data.canRetry}<form method="POST" action="?/retry" use:enhance class="mt-4"><input type="hidden" name="requestId" value={data.record.requestId} /><Button type="submit" variant="outline">{text.retryOriginal}</Button></form>{/if}
  <section class="mt-6 rounded-xl border bg-card p-4"><ResponsiveMetadata {items} dense wrapperClass="md:grid-cols-2 md:gap-x-6" /></section>
  <section class="mt-5 rounded-xl border bg-card p-4"><h2 class="text-sm font-medium">{text.reason}</h2><p class="mt-2 whitespace-pre-wrap break-words text-sm text-muted-foreground">{data.record.reason}</p></section>
  {#if data.record.status==='applied'}
  <div class="mt-5 overflow-x-auto rounded-xl border bg-card"><table class="w-full text-left text-sm">
    <thead class="border-b bg-muted/40"><tr><th scope="col" class="px-4 py-2.5 font-medium">{text.name}</th><th scope="col" class="px-4 py-2.5 font-medium">{text.before}</th><th scope="col" class="px-4 py-2.5 font-medium">{text.after}</th></tr></thead>
    <tbody>{#each keys as key}<tr class="border-b last:border-0"><th scope="row" class="px-4 py-2.5 font-medium">{key==='enabled'?text.policyStatus:label(key, page.data.locale)}</th><td class="max-w-72 px-4 py-2.5 [overflow-wrap:anywhere]">{display(before?.[key], key)}</td><td class="max-w-72 px-4 py-2.5 [overflow-wrap:anywhere]">{display(after[key], key)}</td></tr>{/each}</tbody>
  </table></div>
  {#if after.planSnapshot}<details class="mt-5 rounded-xl border bg-card p-4"><summary class="cursor-pointer text-sm font-medium">{text.planSnapshot}</summary><pre class="mt-3 overflow-x-auto text-xs">{JSON.stringify(after.planSnapshot, null, 2)}</pre></details>{/if}
  {/if}
  <div class="mt-5"><Button variant="outline" href={`/${data.record.action === 'punch.policy' ? 'nodes' : data.record.action.startsWith('user.') ? 'users' : data.record.action === 'plan.update' ? 'plans' : 'projects'}/${encodeURIComponent(data.record.targetId)}`}>{text.details}</Button></div>
</div>
