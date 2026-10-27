<script lang="ts">
  import { page } from '$app/state';
  import Sprout from '@lucide/svelte/icons/sprout';
  import { messages } from '#lib/i18n.js';
  import { formatBytes } from '#lib/upload.js';
  import { Button } from '@q2q/ui/button';
  import DownloadGrantPanel from '#lib/components/download-grant-panel.svelte';
  import type { PageProps } from './$types';
  let { data }: PageProps = $props();
  const text = $derived(messages[page.data.locale]);
</script>
<svelte:head><title>{data.seed.name} · {text.seeds} · Q2Q</title></svelte:head>
<div class="mx-auto w-full max-w-4xl px-4 py-8 sm:px-7">
  <div class="mb-5 flex items-center gap-3"><Sprout class="size-7 shrink-0 text-primary" /><h1 class="min-w-0 break-all text-2xl font-medium tracking-tight">{data.seed.name}</h1></div>
  <dl class="grid gap-5 rounded-xl border bg-card p-5 text-sm sm:grid-cols-2">
    <div class="sm:col-span-2"><dt class="text-[13px] text-muted-foreground">{text.seedVersion}</dt><dd class="mt-2 break-all font-mono text-xs">{data.seed.id}</dd></div>
    <div class="sm:col-span-2"><dt class="text-[13px] text-muted-foreground">{text.seedContentIdentity}</dt><dd class="mt-2 break-all font-mono text-xs">{data.seed.infoHash}</dd></div>
    <div><dt class="text-[13px] text-muted-foreground">{text.size}</dt><dd class="mt-2">{formatBytes(data.seed.size, page.data.locale)}</dd></div>
    <div><dt class="text-[13px] text-muted-foreground">{text.fileStatus}</dt><dd class="mt-2">{text[({ creating: 'publicationCreating', published: 'publicationPublished', failed: 'publicationFailed', withdrawn: 'publicationWithdrawn' } as const)[data.seed.status]]}</dd></div>
    <div><dt class="text-[13px] text-muted-foreground">{text.seedSourceFile}</dt><dd class="mt-2">{#if data.sourceAvailable}<a href={`/library/${data.seed.fileId}`} class="text-primary hover:underline">{data.seed.name}</a>{:else}{data.seed.name}{/if}</dd></div>
    <div><dt class="text-[13px] text-muted-foreground">{text.fileCreatedAt}</dt><dd class="mt-2">{new Intl.DateTimeFormat(page.data.locale, { dateStyle: 'medium', timeStyle: 'short' }).format(data.seed.createdAt)}</dd></div>
  </dl>
  <section class="mt-5 rounded-xl border bg-card p-5"><h2 class="text-sm font-medium">{text.network}</h2><p class="mt-2 text-sm">{data.seed.trackUrl ? data.seed.networkMode === 'custom' ? text.networkCustom : text.networkPlatform : text.networkNotConfigured}</p>{#if data.seed.trackUrl}<p class="mt-2 break-all font-mono text-xs text-muted-foreground">{data.seed.trackUrl}</p>{/if}<p class="mt-3 text-[13px] text-muted-foreground">{text.networkFuture}</p></section>
  {#if data.canManage && data.seed.status === 'published'}<div class="mt-5"><DownloadGrantPanel publicationId={data.seed.id} name={data.seed.name} infoHash={data.seed.infoHash} networkMode={data.seed.networkMode} trackUrl={data.seed.trackUrl} /></div>{/if}
  <div class="mt-5 rounded-xl border bg-card p-5 text-sm text-muted-foreground">{text.seedStatisticsPending}</div>
</div>
