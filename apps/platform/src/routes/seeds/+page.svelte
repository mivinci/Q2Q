<script lang="ts">
  import { page } from '$app/state';
  import Sprout from '@lucide/svelte/icons/sprout';
  import { messages } from '#lib/i18n.js';
  import { formatBytes } from '#lib/upload.js';
  import SeedInspector from '#lib/components/seed-inspector.svelte';
  import type { PageProps } from './$types';
  let { data }: PageProps = $props();
  const text = $derived(messages[page.data.locale]);
</script>
<svelte:head><title>{text.seeds} · Q2Q</title></svelte:head>
<div class="mx-auto max-w-5xl px-4 py-8 sm:px-7">
  <div class="mb-7 flex flex-wrap items-center justify-between gap-4"><div><h1 class="text-2xl font-medium tracking-tight">{text.seeds}</h1><p class="mt-2 text-sm text-muted-foreground">{text.seedListDescription}</p></div><SeedInspector seeds={data.seeds} /></div>
  <ul class="divide-y rounded-xl border bg-card px-4">
    {#each data.seeds as seed (seed.id)}
      <li><a href={`/seeds/${seed.id}`} class="flex items-center gap-3 rounded-lg px-2 py-5 hover:bg-muted/50 focus-visible:outline-ring"><Sprout class="size-5 shrink-0 text-primary" /><div class="min-w-0 flex-1"><p class="truncate text-sm font-medium">{seed.name}</p><p class="mt-1 break-all font-mono text-xs text-muted-foreground">{seed.id}</p><p class="mt-2 text-xs text-muted-foreground">{formatBytes(seed.size, page.data.locale)} · {text[({ creating: 'publicationCreating', published: 'publicationPublished', failed: 'publicationFailed', withdrawn: 'publicationWithdrawn' } as const)[seed.status]]}</p></div></a></li>
    {:else}<li class="py-12 text-center text-sm text-muted-foreground">{text.seedEmpty}</li>{/each}
  </ul>
</div>
