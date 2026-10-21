<script lang="ts">
  import { page } from '$app/state';
  import File from '@lucide/svelte/icons/file';
  import { messages } from '#lib/i18n.js';
  import { fileStatusLabel, formatBytes } from '#lib/upload.js';

  type FileListEntry = {
    id: string;
    name: string;
    size: number;
    status: string;
    uploadStatus: Parameters<typeof fileStatusLabel>[1];
    processingStatus: Parameters<typeof fileStatusLabel>[2];
    updatedAt: number;
  };

  let { files }: { files: FileListEntry[] } = $props();
  const text = $derived(messages[page.data.locale]);
  const updatedFormat = $derived(new Intl.DateTimeFormat(page.data.locale, { dateStyle: 'medium' }));
</script>

<div role="table" aria-label={text.contentList} class="overflow-hidden rounded-xl border bg-card">
  <div role="rowgroup">
    <div role="row" class="grid h-11 grid-cols-[minmax(0,1fr)_auto] items-center border-b px-2 text-sm text-muted-foreground sm:grid-cols-[minmax(0,1.8fr)_minmax(6.5rem,0.8fr)_minmax(4rem,0.55fr)_minmax(7rem,0.85fr)]">
      <span role="columnheader" class="flex items-center gap-2"><span class="size-4 shrink-0" aria-hidden="true"></span><span>{text.name}</span></span>
      <span role="columnheader">{text.fileStatus}</span>
      <span role="columnheader" class="hidden sm:block">{text.size}</span>
      <span role="columnheader" class="hidden sm:block">{text.updated}</span>
    </div>
  </div>
  <div role="rowgroup" class="divide-y">
    {#each files as file (file.id)}
      <a role="row" href={`/library/${file.id}`} class="grid min-h-11 grid-cols-[minmax(0,1fr)_auto] items-center gap-3 px-2.5 py-2 transition-colors duration-100 hover:bg-muted/40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset sm:grid-cols-[minmax(0,1.8fr)_minmax(6.5rem,0.8fr)_minmax(4rem,0.55fr)_minmax(7rem,0.85fr)]">
        <span role="cell" class="flex min-w-0 items-center gap-2.5"><File class="size-4 shrink-0 text-muted-foreground" /><span class="truncate text-sm">{file.name}</span></span>
        <span role="cell" class="text-[13px] text-muted-foreground">{text[fileStatusLabel(file.status, file.uploadStatus, file.processingStatus)]}</span>
        <span role="cell" class="hidden text-xs tabular-nums text-muted-foreground sm:block">{formatBytes(file.size, page.data.locale)}</span>
        <span role="cell" class="hidden text-xs text-muted-foreground sm:block">{updatedFormat.format(file.updatedAt)}</span>
      </a>
    {/each}
  </div>
</div>
