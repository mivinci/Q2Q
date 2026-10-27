<script lang="ts">
 import { page } from '$app/state';
 import { messages,type MessageKey } from '#lib/messages.js';
 import { bytes,date,label } from '#lib/format.js';
 import { Button } from '@q2q/ui/button';
 import AuditList from '#lib/components/audit-list.svelte';
 import Table from '#lib/components/record-table.svelte';
 import type { PageProps } from './$types';
 let {data}:PageProps=$props();const text=$derived(messages[page.data.locale]);
 const description=$derived(`${data.section}Description` as MessageKey);
 function listLink(n:number){return `/${data.section}?${new URLSearchParams({q:data.q,page:String(n)})}`;}
</script>
<svelte:head><title>{text[data.section]} · Q2Q Admin</title></svelte:head>
<div class="mx-auto w-full max-w-5xl px-4 pt-6 pb-8 sm:px-7">
 <h1 class="text-2xl font-medium tracking-tight">{text[data.section]}</h1><p class="mt-1 text-sm text-muted-foreground">{text[description]}</p>
 {#if data.section==='users'||data.section==='projects'}
 <form method="GET" class="mt-6 flex flex-wrap gap-2"><input name="q" value={data.q} maxlength="100" aria-label={text.search} placeholder={text.searchPlaceholder} class="h-10 min-w-0 flex-1 rounded-lg border bg-background px-3 outline-none focus-visible:ring-2 focus-visible:ring-ring" /><Button type="submit" variant="outline" class="h-10">{text.search}</Button></form>
 <div class="mt-4"><Table rows={data.rows} columns={data.columns} section={data.section} /></div>
 <nav class="mt-4 flex justify-end gap-2" aria-label={text.navigation}>{#if data.page>1}<Button href={listLink(data.page-1)} variant="outline">{text.previous}</Button>{/if}{#if data.hasNext}<Button href={listLink(data.page+1)} variant="outline">{text.next}</Button>{/if}</nav>
 {:else if data.section==='plans'}
 <p class="mt-6 text-sm text-muted-foreground">{text.plansNotice}</p>
 <div class="mt-4 grid gap-4 lg:grid-cols-3">{#each data.plans as plan}<article class="rounded-xl border bg-card p-5"><h2 class="text-lg font-medium capitalize">{plan.id}</h2><div class="mt-3"><Button href={`/plans/${plan.id}`} variant="outline">{text.editPlan}</Button></div><dl class="mt-5 space-y-4"><div><dt class="text-sm text-muted-foreground">{text.maxFileSizeBytes}</dt><dd class="mt-1 font-medium">{bytes(plan.maxFileSizeBytes)}</dd></div><div><dt class="text-sm text-muted-foreground">{text.storageBytes}</dt><dd class="mt-1 font-medium">{bytes(plan.storageBytes)}</dd></div></dl><ul class="mt-5 space-y-2 text-sm">{#each plan.features as feature}<li>{label(feature,page.data.locale)}</li>{/each}</ul></article>{/each}</div>
 {:else if data.section==='nodes'}
 <p class="mt-6 text-sm text-muted-foreground">{text.nodesNotice}</p>
 {#if data.unavailable}<p role="status" class="mt-4 rounded-xl border bg-card p-5 text-sm">{text.unavailable}</p>{:else}<div class="mt-4"><Table rows={data.rows} columns={data.columns} section="nodes" /></div><nav aria-label={text.navigation} class="mt-4 flex justify-end gap-2">{#if page.url.searchParams.has('after')}<Button href="/nodes" variant="outline">{text.first}</Button>{/if}{#if data.nextCursor}<Button href={`/nodes?after=${encodeURIComponent(data.nextCursor)}`} variant="outline">{text.next}</Button>{/if}</nav>{/if}
 {:else if data.audit}<AuditList data={data.audit} />{/if}
 {#if data.section!=='audit'}<p class="mt-6 text-xs text-muted-foreground">{text.observedAt} · {date(data.observedAt,page.data.locale)}</p>{/if}
</div>
