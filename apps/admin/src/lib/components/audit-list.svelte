<script lang="ts">
  import { page } from '$app/state';
  import { messages } from '#lib/messages.js';
  import { date, label } from '#lib/format.js';
  import { Button } from '@q2q/ui/button';
  import type { listAudit } from '#lib/server/audit.js';
  let { data }: { data: Awaited<ReturnType<typeof listAudit>> } = $props();
  const text = $derived(messages[page.data.locale]);
  const inputClass = 'min-h-10 min-w-0 rounded-lg border bg-background px-3 outline-none focus-visible:ring-2 focus-visible:ring-ring';
  function link(n: number) { const params = new URLSearchParams(page.url.searchParams.toString()); params.set('page', String(n)); return `/audit?${params}`; }
</script>
<form method="GET" class="mt-6 grid items-end gap-3 sm:grid-cols-2 lg:grid-cols-4">
  <input name="q" aria-label={text.auditSearch} placeholder={text.auditSearch} value={data.q} maxlength="200" class={inputClass} />
  <select name="action" aria-label={text.action} value={data.action} class={inputClass}>
    <option value="">{text.allActions}</option><option value="plan.update">{text['plan.update']}</option><option value="subscription.set">{text['subscription.set']}</option><option value="user.disable">{text['user.disable']}</option><option value="user.restore">{text['user.restore']}</option><option value="user.sessions.revoke">{text['user.sessions.revoke']}</option><option value="punch.policy">{text['punch.policy']}</option>
  </select>
  <label class="text-sm">{text.fromDate}<input type="date" name="from" value={data.from} class={`mt-2 w-full ${inputClass}`} /></label>
  <label class="text-sm">{text.toDate}<input type="date" name="to" value={data.to} class={`mt-2 w-full ${inputClass}`} /></label>
  <div><Button type="submit" variant="outline">{text.search}</Button></div>
</form>
<div class="mt-4 overflow-x-auto rounded-xl border bg-card">
  <table class="w-full text-left text-sm">
    <thead class="border-b bg-muted/40"><tr>{#each [text.action, text.targetId, text.actorEmail, text.operationStatus, text.createdAt] as title}<th scope="col" class="whitespace-nowrap px-4 py-3 font-medium text-muted-foreground">{title}</th>{/each}</tr></thead>
    <tbody>{#each data.rows as row}<tr class="border-b last:border-0"><td class="px-4 py-3"><a class="font-medium text-primary hover:underline" href={`/audit/${row.requestId}`}>{label(row.action, page.data.locale)}</a></td><td class="px-4 py-3">{row.targetId}</td><td class="px-4 py-3">{row.actorEmail}</td><td class="px-4 py-3">{label(row.status,page.data.locale)}</td><td class="whitespace-nowrap px-4 py-3">{date(row.createdAt, page.data.locale)}</td></tr>{:else}<tr><td colspan="5" class="px-4 py-10 text-center text-muted-foreground">{text.empty}</td></tr>{/each}</tbody>
  </table>
</div>
<nav class="mt-4 flex justify-end gap-2" aria-label={text.navigation}>{#if data.page > 1}<Button href={link(data.page - 1)} variant="outline">{text.previous}</Button>{/if}{#if data.hasNext}<Button href={link(data.page + 1)} variant="outline">{text.next}</Button>{/if}</nav>
