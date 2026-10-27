<script lang="ts">
 import UserControls from '#lib/components/user-controls.svelte';
 import SubscriptionEditor from '#lib/components/subscription-editor.svelte';
 import { page } from '$app/state';
 import { messages } from '#lib/messages.js';
 import { bytes,date,label } from '#lib/format.js';
 import { Button } from '@q2q/ui/button';
 import ResponsiveMetadata from '@q2q/ui/responsive-metadata';
 import Table from '#lib/components/record-table.svelte';
 import type { PageProps } from './$types';
 let {data,form}:PageProps=$props(); const locale=$derived(page.data.locale); const text=$derived(messages[locale]);
 const items=$derived(data.fields.map(field=>({label:text[field.label],valueClass:field.key === 'disabledReason' ? 'min-w-0 break-words' : '',value:data.record[field.key]===null?'—':field.format==='date'?date(data.record[field.key],locale):field.format==='bytes'?bytes(Number(data.record[field.key])):field.format==='label'?label(String(data.record[field.key]),locale):String(data.record[field.key]??'—')})));
</script>
<svelte:head><title>{data.record.name} · Q2Q Admin</title></svelte:head>
<div class="mx-auto w-full max-w-5xl px-4 pt-6 pb-8 sm:px-7">
 <h1 class="break-words text-2xl font-medium tracking-tight">{data.record.name}</h1><p class="mt-1 text-sm text-muted-foreground">{data.section==='users'?text.userDetails:text.projectDetails}</p>
 <section class="mt-6 rounded-xl border bg-card p-5"><ResponsiveMetadata {items} wrapperClass="sm:grid-cols-2 sm:gap-6" /></section>
 <h2 class="mt-8 mb-4 text-base font-medium">{data.section==='users'?text.projectMembership:text.members}</h2>
 <Table rows={data.related} columns={data.columns} section={data.relatedSection} />
 <nav class="mt-4 flex justify-end gap-2" aria-label={text.navigation}>{#if data.page>1}<Button href={`?page=${data.page-1}`} variant="outline">{text.previous}</Button>{/if}{#if data.hasNext}<Button href={`?page=${data.page+1}`} variant="outline">{text.next}</Button>{/if}</nav>
 {#if data.section === 'users'}<UserControls record={data.record} requestId={data.requestId} revokeRequestId={data.revokeRequestId} {form} />{/if}
 {#if data.section === 'projects'}<SubscriptionEditor subscription={data.subscription} definitions={data.definitions} requestId={data.requestId} record={data.record} {form} />{/if}
 <p class="mt-6 text-xs text-muted-foreground">{text.observedAt} · {date(data.observedAt,locale)}</p>
</div>
