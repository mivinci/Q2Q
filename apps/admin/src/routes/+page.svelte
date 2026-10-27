<script lang="ts">
 import { page } from '$app/state';
 import { messages } from '#lib/messages.js';
 import { date } from '#lib/format.js';
 import type { PageProps } from './$types';
 let {data}:PageProps=$props(); const text=$derived(messages[page.data.locale]);
</script>
<svelte:head><title>{text.overview} · Q2Q Admin</title></svelte:head>
<div class="mx-auto w-full max-w-5xl px-4 pt-5 pb-7 sm:px-7">
 <h1 class="text-2xl font-medium tracking-tight">{text.overview}</h1><p class="mt-1 text-sm text-muted-foreground">{text.overviewDescription}</p>
 <div class="mt-6 grid gap-3 sm:grid-cols-2">
 {#each [{name:text.users,count:data.summary.users,href:'/users'},{name:text.projects,count:data.summary.projects,href:'/projects'}] as item}<a href={item.href} class="flex min-h-24 flex-col justify-between rounded-xl border bg-card p-4 hover:bg-muted/30"><span class="text-[13px] text-muted-foreground">{item.name}</span><span class="mt-2 text-2xl font-medium tabular-nums">{item.count}</span></a>{/each}
 </div>
 <h2 class="mt-6 text-base font-medium">{text.plans}</h2>
 <div class="mt-3 grid gap-3 sm:grid-cols-3">{#each data.summary.plans as plan}<article class="rounded-xl border bg-card p-4"><p class="text-sm font-medium capitalize">{plan.id}</p><p class="mt-2 text-2xl tabular-nums">{plan.count}<span class="ml-2 text-xs text-muted-foreground">{text.projects}</span></p></article>{/each}</div>
 <p class="mt-6 text-xs text-muted-foreground">{text.observedAt} · {date(data.observedAt,page.data.locale)}</p>
</div>
