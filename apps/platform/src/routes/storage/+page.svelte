<script lang="ts">
  import Cloud from '@lucide/svelte/icons/cloud';
  import HardDrive from '@lucide/svelte/icons/hard-drive';
  import Check from '@lucide/svelte/icons/check';
  import { messages } from '#lib/i18n.js';
  import { Button } from '#lib/components/ui/button/index.js';
  import type { PageProps } from './$types';
  import StorageUsage from '#lib/components/storage-usage.svelte';
  let { data, form }: PageProps = $props();
  const text = $derived(messages[data.locale]);
  const enabledDate = $derived(data.storageConnection
    ? new Intl.DateTimeFormat(data.locale, { dateStyle: 'medium', timeZone: 'UTC' }).format(data.storageConnection.createdAt)
    : null);
</script>

<svelte:head><title>{text.storage} · Q2Q</title></svelte:head>
<div class="mx-auto w-full max-w-4xl px-4 py-8 sm:px-7">
  <h1 class="text-2xl font-medium tracking-tight">{text.storage}</h1>
  <p class="mt-2 text-sm text-muted-foreground">{text.storageDescription}</p>
  {#if form?.error}<p role="alert" class="mt-5 text-sm text-destructive">{text[form.error]}</p>{/if}
  {#if !data.storageAvailable}<p role="status" class="mt-5 text-sm text-muted-foreground">{text.storageUnavailable}</p>{/if}

  <section aria-labelledby="platform-storage" class="mt-8 overflow-hidden rounded-xl border bg-card">
    <div class="flex items-start gap-4 p-5 sm:p-6">
      <span class="flex size-10 shrink-0 items-center justify-center rounded-xl bg-primary/10 text-primary"><Cloud class="size-5" aria-hidden="true" /></span>
      <div class="min-w-0 flex-1">
        <div class="flex flex-wrap items-center justify-between gap-3">
          <h2 id="platform-storage" class="text-sm font-medium">{text.platformStorage}</h2>
          <span class={data.storageConnection ? 'inline-flex items-center gap-1 rounded-full bg-emerald-500/10 px-2 py-1 text-xs text-emerald-700 dark:text-emerald-400' : 'rounded-full bg-muted px-2 py-1 text-xs text-muted-foreground'}>
            {#if data.storageConnection}<Check class="size-3" aria-hidden="true" />{/if}
            {data.storageConnection ? text.storageEnabled : text.storageNotEnabled}
          </span>
        </div>
        <p class="mt-2 text-[13px] leading-relaxed text-muted-foreground">{text.platformStorageDescription}</p>
      </div>
    </div>
    <dl class="grid gap-5 border-t px-5 py-5 text-[13px] sm:grid-cols-2 sm:px-6 lg:grid-cols-3">
      <div><dt class="text-muted-foreground">{text.storageProvider}</dt><dd class="mt-2">{text.s3Compatible}</dd></div>
      <div><dt class="text-muted-foreground">{text.storageScopeLabel}</dt><dd class="mt-2">{text.storageScope}</dd></div>
      {#if enabledDate}<div><dt class="text-muted-foreground">{text.storageEnabledAt}</dt><dd class="mt-2">{enabledDate}</dd></div>{/if}
    </dl>
    <div class="border-t px-5 py-4 sm:px-6"><StorageUsage usage={data.storageUsage} limit={data.entitlements.limits.storageBytes} /><a class="mt-2 inline-block text-[13px] text-primary" href="/usage">{text.viewPlans}</a></div>
    <div class="flex flex-col items-start justify-between gap-4 border-t bg-muted/30 px-5 py-4 sm:flex-row sm:items-center sm:px-6">
      <p class="text-[13px] leading-relaxed text-muted-foreground">{data.storageConnection ? text.storageReadyDescription : text.storageStartDescription}</p>
      {#if !data.storageConnection}
        {#if data.currentProject?.role === 'owner'}
          <form method="POST" action="?/enable" class="shrink-0">
            <input type="hidden" name="projectId" value={data.currentProject.id} />
            <Button type="submit" disabled={!data.storageAvailable}>{text.enableStorage}</Button>
          </form>
        {:else}<p class="text-[13px] text-muted-foreground">{text.storageOwnerRequired}</p>{/if}
      {/if}
    </div>
  </section>

  <section aria-labelledby="own-storage" class="mt-5 flex items-start gap-4 rounded-xl border p-5 sm:p-6">
    <span class="flex size-10 shrink-0 items-center justify-center rounded-xl bg-muted text-muted-foreground"><HardDrive class="size-5" aria-hidden="true" /></span>
    <div class="min-w-0 flex-1">
      <div class="flex flex-wrap items-center justify-between gap-3"><h2 id="own-storage" class="text-sm font-medium">{text.ownStorage}</h2><span class="text-xs text-muted-foreground">{text.comingSoon}</span></div>
      <p class="mt-2 text-[13px] leading-relaxed text-muted-foreground">{text.ownStorageDescription}</p>
    </div>
  </section>
</div>
