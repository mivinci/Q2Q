<script lang="ts">
  import { page } from '$app/state';
  import { messages } from '#lib/messages.js';
  import { date, label } from '#lib/format.js';
  import { Button } from '@q2q/ui/button';
  import { Input } from '@q2q/ui/input';
  import * as Select from '@q2q/ui/select';
  import DatePicker from '@q2q/ui/date-picker';
  import Search from '@lucide/svelte/icons/search';
  import type { listAudit } from '#lib/server/audit.js';
  let { data }: { data: Awaited<ReturnType<typeof listAudit>> } = $props();
  const text = $derived(messages[page.data.locale]);
  const actions = ['plan.update','subscription.set','user.disable','user.restore','user.sessions.revoke','punch.policy'] as const;
  let action=$derived(data.action||'all');
  function link(n: number) { const params = new URLSearchParams(page.url.searchParams.toString()); params.set('page', String(n)); return `/audit?${params}`; }
</script>
<form method="GET" class="mt-6 grid grid-cols-1 items-end gap-4 rounded-xl border bg-card p-4 sm:grid-cols-2 xl:grid-cols-[minmax(14rem,1.5fr)_minmax(10rem,1fr)_minmax(10rem,1fr)_minmax(10rem,1fr)_auto]">
  <div class="min-w-0 space-y-2 sm:col-span-2 xl:col-span-1">
    <label for="audit-query" class="text-sm font-medium">{text.search}</label>
    <Input id="audit-query" name="q" placeholder={text.auditSearch} value={data.q} maxlength={200} class="h-10 bg-background px-3" />
  </div>
  <div class="min-w-0 space-y-2">
    <label for="audit-action" class="text-sm font-medium">{text.action}</label>
    <input type="hidden" name="action" value={action==='all'?'':action} />
    <Select.Root type="single" bind:value={action}>
      <Select.Trigger id="audit-action" class="h-10 w-full bg-background px-3 data-[size=default]:h-10"><span class="truncate">{action==='all'?text.allActions:label(action,page.data.locale)}</span></Select.Trigger>
      <Select.Content><Select.Item value="all">{text.allActions}</Select.Item>{#each actions as item}<Select.Item value={item}>{text[item]}</Select.Item>{/each}</Select.Content>
    </Select.Root>
  </div>
  <div class="min-w-0 space-y-2">
    <label for="audit-from" class="text-sm font-medium">{text.fromDate}</label>
    <DatePicker id="audit-from" name="from" value={data.from} locale={page.data.locale} placeholder={text.selectDate} clearLabel={text.clearDate} />
  </div>
  <div class="min-w-0 space-y-2">
    <label for="audit-to" class="text-sm font-medium">{text.toDate}</label>
    <DatePicker id="audit-to" name="to" value={data.to} locale={page.data.locale} placeholder={text.selectDate} clearLabel={text.clearDate} />
  </div>
  <Button type="submit" variant="outline" class="h-10 gap-2 px-4 sm:justify-self-end"><Search class="size-4" aria-hidden="true" />{text.search}</Button>
</form>
<div class="mt-4 overflow-x-auto rounded-xl border bg-card">
  <table class="w-full text-left text-sm">
    <thead class="border-b bg-muted/40"><tr>{#each [text.action, text.targetId, text.actorEmail, text.operationStatus, text.createdAt] as title}<th scope="col" class="whitespace-nowrap px-4 py-2.5 font-medium text-muted-foreground">{title}</th>{/each}</tr></thead>
    <tbody>{#each data.rows as row}<tr class="border-b last:border-0"><td class="max-w-72 px-4 py-2.5 [overflow-wrap:anywhere]"><a class="font-medium text-primary hover:underline" href={`/audit/${row.requestId}`}>{label(row.action, page.data.locale)}</a></td><td class="max-w-72 px-4 py-2.5 [overflow-wrap:anywhere]">{row.targetId}</td><td class="max-w-72 px-4 py-2.5 [overflow-wrap:anywhere]">{row.actorEmail}</td><td class="max-w-72 px-4 py-2.5 [overflow-wrap:anywhere]">{label(row.status,page.data.locale)}</td><td class="whitespace-nowrap px-4 py-2.5">{date(row.createdAt, page.data.locale)}</td></tr>{:else}<tr><td colspan="5" class="px-4 py-10 text-center text-muted-foreground">{text.empty}</td></tr>{/each}</tbody>
  </table>
</div>
<nav class="mt-4 flex justify-end gap-2" aria-label={text.navigation}>{#if data.page > 1}<Button href={link(data.page - 1)} variant="outline">{text.previous}</Button>{/if}{#if data.hasNext}<Button href={link(data.page + 1)} variant="outline">{text.next}</Button>{/if}</nav>
