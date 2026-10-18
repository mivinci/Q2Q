<script lang="ts">
  import { page } from '$app/state';
  import ArrowRight from '@lucide/svelte/icons/arrow-right';
  import File from '@lucide/svelte/icons/file';
  import Sprout from '@lucide/svelte/icons/sprout';
  import { Button } from '#lib/components/ui/button/index.js';
  import StorageUsage from '#lib/components/storage-usage.svelte';
  import { messages } from '#lib/i18n.js';
  import { fileStatusLabel, formatBytes } from '#lib/upload.js';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();
  const text = $derived(messages[page.data.locale]);
  const projectFileCount = $derived(new Intl.NumberFormat(page.data.locale).format(data.fileCount));
  const primaryHref = $derived(data.storageEnabled ? '/library#upload' : '/storage');
  const primaryLabel = $derived(data.storageEnabled
    ? text.upload
    : data.currentProject.role === 'owner' ? text.enableStorage : text.storage);
</script>

<svelte:head>
  <title>{text.overview} · Q2Q</title>
  <meta name="description" content={text.overviewDescription} />
</svelte:head>

<div class="mx-auto w-full max-w-5xl px-4 pt-6 pb-8 sm:px-7">
  <header class="flex flex-wrap items-start justify-between gap-5">
    <div class="min-w-0">
      <h1 class="text-2xl font-medium tracking-tight">{text.overview}</h1>
      <p class="mt-1 text-sm text-muted-foreground">{text.overviewDescription}</p>
    </div>
    <div class="flex flex-wrap gap-2">
      <Button href={primaryHref} disabled={data.storageEnabled && !data.uploadReady}>
        {#if data.storageEnabled}<File class="size-4" aria-hidden="true" />{/if}
        {primaryLabel}
      </Button>
      <Button href="/seeds" variant="outline"><Sprout class="size-4" aria-hidden="true" />{text.inspectSeed}</Button>
    </div>
  </header>

  {#if data.storageEnabled && !data.uploadReady}
    <p role="status" class="mt-5 text-[13px] text-muted-foreground">{text.storageUnavailable}</p>
  {/if}

  <section aria-label={text.usageSummary} class="mt-7 grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
    <article class="flex min-h-36 flex-col justify-between rounded-xl border bg-card p-4">
      <div>
        <p class="text-[13px] text-muted-foreground">{text.projectFiles}</p>
        <p class="mt-2 text-2xl font-medium tabular-nums">{projectFileCount}</p>
      </div>
      <a href="/library" class="inline-flex items-center gap-1 text-[13px] text-primary">
        {text.viewAll}<ArrowRight class="size-3.5" aria-hidden="true" />
      </a>
    </article>

    <article class="min-h-36 rounded-xl border bg-card p-4">
      <StorageUsage usage={data.storageUsage} limit={data.entitlements.limits.storageBytes} />
    </article>

    <article class="flex min-h-36 flex-col justify-between rounded-xl border bg-card p-4">
      <div>
        <p class="text-[13px] text-muted-foreground">{text.currentPlan}</p>
        <p class="mt-2 text-xl font-medium">{text[data.planNameKey]}</p>
      </div>
      <a href="/usage" class="inline-flex items-center gap-1 text-[13px] text-primary">
        {text.viewPlans}<ArrowRight class="size-3.5" aria-hidden="true" />
      </a>
    </article>
  </section>

  <section aria-labelledby="recent-files-title" class="mt-9">
    <div class="mb-3 flex items-center justify-between gap-4">
      <h2 id="recent-files-title" class="text-base font-medium">{text.recentFiles}</h2>
      {#if data.recentFiles.length}
        <a href="/library" class="inline-flex items-center gap-1 text-[13px] text-primary">
          {text.viewAll}<ArrowRight class="size-3.5" aria-hidden="true" />
        </a>
      {/if}
    </div>

    {#if data.recentFiles.length}
      <ul class="overflow-hidden rounded-xl border bg-card">
        {#each data.recentFiles as file (file.id)}
          <li class="border-b last:border-b-0">
            <a href={`/library/${file.id}`} class="grid min-h-16 grid-cols-[minmax(0,1fr)_auto] items-center gap-x-4 gap-y-1 px-4 py-3 transition-colors hover:bg-muted/40 focus-visible:outline-ring sm:grid-cols-[minmax(0,2fr)_minmax(8rem,1fr)_minmax(5rem,auto)_minmax(7rem,auto)]">
              <span class="flex min-w-0 items-center gap-3">
                <File class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
                <span class="truncate text-sm">{file.name}</span>
              </span>
              <span class="text-[13px] text-muted-foreground">{text[fileStatusLabel(file.status, file.uploadStatus, file.processingStatus)]}</span>
              <span class="hidden text-[13px] tabular-nums text-muted-foreground sm:block">{formatBytes(file.size, page.data.locale)}</span>
              <span class="hidden text-right text-[13px] text-muted-foreground sm:block">{new Intl.DateTimeFormat(page.data.locale, { dateStyle: 'medium' }).format(file.updatedAt)}</span>
            </a>
          </li>
        {/each}
      </ul>
    {:else}
      <div class="rounded-xl border border-dashed px-5 py-8 text-center">
        <File class="mx-auto size-7 text-muted-foreground" strokeWidth={1.5} aria-hidden="true" />
        <h3 class="mt-3 text-sm font-medium">{text.homeNoFiles}</h3>
        <p class="mx-auto mt-1 max-w-md text-[13px] leading-relaxed text-muted-foreground">
          {data.storageEnabled ? text.homeNoFilesDescription : text.homeStorageFirst}
        </p>
        <Button href={primaryHref} variant="outline" class="mt-4" disabled={data.storageEnabled && !data.uploadReady}>
          {primaryLabel}
        </Button>
      </div>
    {/if}
  </section>
</div>
