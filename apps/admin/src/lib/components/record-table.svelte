<script lang="ts">
 import { page } from '$app/state';
 import { messages } from '#lib/messages.js';
 import { bytes,date,label } from '#lib/format.js';
 import type { Column,Row } from '#lib/server/queries.js';
 let {rows,columns,section}: {rows:Row[];columns:Column[];section?:string}=$props();
 const locale=$derived(page.data.locale); const text=$derived(messages[locale]);
</script>
<div class="overflow-x-auto rounded-xl border bg-card">
 <table class="w-full text-left text-sm">
  <thead class="border-b bg-muted/40"><tr>{#each columns as column}<th scope="col" class="whitespace-nowrap px-4 py-2.5 font-medium text-muted-foreground">{text[column.label]}</th>{/each}</tr></thead>
  <tbody>{#each rows as row}<tr class="border-b last:border-0 hover:bg-muted/30">{#each columns as column,index}<td class="max-w-72 px-4 py-2.5 [overflow-wrap:anywhere]">{#if index===0 && section}<a class="font-medium text-primary hover:underline" href={`/${section}/${encodeURIComponent(String(row.id))}`}>{row[column.key] || row.id}</a>{:else if column.format==='date'}{date(row[column.key],locale)}{:else if column.format==='bytes'}{bytes(Number(row[column.key]))}{:else if column.format==='label'}{row[column.key]===null?'—':label(String(row[column.key]),locale)}{:else}{row[column.key] ?? '—'}{/if}</td>{/each}</tr>{:else}<tr><td colspan={columns.length} class="px-4 py-10 text-center text-muted-foreground">{text.empty}</td></tr>{/each}</tbody>
 </table>
</div>
