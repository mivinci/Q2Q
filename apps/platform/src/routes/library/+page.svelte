<script lang="ts">
  import { fileStatusLabel, formatBytes } from '#lib/upload.js';
  import File from '@lucide/svelte/icons/file';
  import FolderClosed from '@lucide/svelte/icons/folder-closed';
  import { page } from '$app/state';
  import { messages } from '#lib/i18n.js';
  import UploadPanel from '#lib/components/upload-panel.svelte';
  import type { PageProps } from './$types';
  let { data, form }: PageProps = $props();
  const text = $derived(messages[page.data.locale]);
</script>

<svelte:head>
  <title>{text.library} · Q2Q</title>
  <meta name="description" content={text.metaDescription} />
</svelte:head>

<div class="mx-auto w-full max-w-5xl px-4 py-8 sm:px-7">
  {#if form?.error}<p role="alert" class="mb-5 text-sm text-destructive">{text[form.error]}</p>{/if}
  {#if data.currentProject}
    {#key data.currentProject.id}
      <UploadPanel projectId={data.currentProject.id} ready={data.uploadReady} storageEnabled={data.storageEnabled} localUploads={data.localUploads} uploads={[]} />
    {/key}
  {/if}
  <section aria-label={text.contentList}>
    <div class="grid grid-cols-[1fr_auto] border-b pb-3 text-[13px] text-muted-foreground sm:grid-cols-[2fr_1fr_1fr_1fr]">
      <span>{text.name}</span><span>{text.fileStatus}</span><span class="hidden sm:block">{text.size}</span><span class="hidden sm:block">{text.updated}</span>
    </div>
    {#if data.files.length}
      <ul class="divide-y border-b">
        {#each data.files as file (file.id)}
          <li><a href={`/library/${file.id}`} class="grid min-h-20 grid-cols-[1fr_auto] items-center gap-3 rounded-lg px-2 py-4 transition-colors hover:bg-muted/50 focus-visible:outline-ring sm:grid-cols-[2fr_1fr_1fr_1fr]">
            <span class="flex min-w-0 items-center gap-3"><File class="size-5 shrink-0 text-muted-foreground" /><span class="truncate text-sm">{file.name}</span></span>
            <span class="text-[13px] text-muted-foreground">{text[fileStatusLabel(file.status, file.uploadStatus, file.processingStatus)]}</span>
            <span class="hidden text-xs tabular-nums text-muted-foreground sm:block">{formatBytes(file.size, page.data.locale)}</span>
            <span class="hidden text-xs text-muted-foreground sm:block">{new Intl.DateTimeFormat(page.data.locale, { dateStyle: 'medium' }).format(file.updatedAt)}</span>
          </a></li>
        {/each}
      </ul>
    {:else}
    <div class="flex min-h-80 flex-col items-center justify-center gap-3 border-b py-12 text-center">
      <FolderClosed class="size-8 text-muted-foreground" strokeWidth={1.25} aria-hidden="true" />
      <h2 class="text-sm font-medium">{text.empty}</h2>
      <p class="max-w-64 text-[13px] text-muted-foreground">{text.emptyDescription}</p>
    </div>
    {/if}
    <p class="mt-3 text-[13px] text-muted-foreground">{text.contentCount(data.files.length)}</p>
    {#if data.files.length === 50}<p class="mt-1 text-[13px] text-muted-foreground">{text.libraryLimit}</p>{/if}
  </section>
</div>
