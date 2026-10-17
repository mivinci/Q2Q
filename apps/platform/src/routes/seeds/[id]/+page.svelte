<script lang="ts">
  import { page } from '$app/state';
  import ArrowLeft from '@lucide/svelte/icons/arrow-left';
  import Sprout from '@lucide/svelte/icons/sprout';
  import { messages } from '#lib/i18n.js';
  import { formatBytes } from '#lib/upload.js';
  import { Button } from '#lib/components/ui/button/index.js';
  import DownloadGrantPanel from '#lib/components/download-grant-panel.svelte';
  import type { PageProps } from './$types';
  let { data }: PageProps = $props();
  const text = $derived(messages[page.data.locale]);
</script>
<svelte:head><title>{data.seed.name} · {text.seeds} · Q2Q</title></svelte:head>
<div class="mx-auto max-w-4xl px-4 py-8 sm:px-7">
  <Button href="/seeds" variant="ghost" size="sm" class="mb-6 -ml-3"><ArrowLeft class="size-4" />{text.seedBack}</Button>
  <div class="mb-7 flex items-start gap-3"><Sprout class="mt-1 size-7 shrink-0 text-primary" /><div class="min-w-0"><p class="mb-1 text-xs text-muted-foreground">{text.seedDetails}</p><h1 class="break-all text-2xl font-medium tracking-tight">{data.seed.name}</h1></div></div>
  <dl class="grid gap-5 rounded-xl border bg-card p-5 text-sm sm:grid-cols-2">
    <div class="sm:col-span-2"><dt class="text-xs text-muted-foreground">{text.seedVersion}</dt><dd class="mt-2 break-all font-mono text-xs">{data.seed.id}</dd></div>
    <div class="sm:col-span-2"><dt class="text-xs text-muted-foreground">{text.seedContentIdentity}</dt><dd class="mt-2 break-all font-mono text-xs">{data.seed.infoHash}</dd></div>
    <div><dt class="text-xs text-muted-foreground">{text.size}</dt><dd class="mt-2">{formatBytes(data.seed.size, page.data.locale)}</dd></div>
    <div><dt class="text-xs text-muted-foreground">{text.fileStatus}</dt><dd class="mt-2">{text[({ creating: 'publicationCreating', published: 'publicationPublished', failed: 'publicationFailed', withdrawn: 'publicationWithdrawn' } as const)[data.seed.status]]}</dd></div>
    <div><dt class="text-xs text-muted-foreground">{text.seedSourceFile}</dt><dd class="mt-2">{#if data.sourceAvailable}<a href={`/library/${data.seed.fileId}`} class="text-primary hover:underline">{data.seed.name}</a>{:else}{data.seed.name}{/if}</dd></div>
    <div><dt class="text-xs text-muted-foreground">{text.fileCreatedAt}</dt><dd class="mt-2">{new Intl.DateTimeFormat(page.data.locale, { dateStyle: 'medium', timeStyle: 'short' }).format(data.seed.createdAt)}</dd></div>
  </dl>
  {#if data.canManage && data.seed.status === 'published'}<div class="mt-6"><DownloadGrantPanel publicationId={data.seed.id} name={data.seed.name} /></div>{/if}
  <div class="mt-6 rounded-xl border bg-card p-5 text-sm text-muted-foreground">{text.seedStatisticsPending}</div>
</div>
