<script lang="ts">
  import FolderClosed from '@lucide/svelte/icons/folder-closed';
  import { page } from '$app/state';
  import { messages } from '#lib/i18n.js';
  import FileList from '#lib/components/file-list.svelte';
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
      <UploadPanel projectId={data.currentProject.id} ready={data.uploadReady} storageEnabled={data.storageEnabled} localUploads={data.localUploads} maxFileSize={data.maxFileSize} storageRemainingBytes={data.storageRemainingBytes} uploads={[]} />
    {/key}
  {/if}
  <section aria-label={text.contentList}>
    {#if data.files.length}
      <FileList files={data.files} />
    {:else}
    <div class="flex min-h-80 flex-col items-center justify-center gap-3 border-b py-12 text-center">
      <FolderClosed class="size-8 text-muted-foreground" strokeWidth={1.25} aria-hidden="true" />
      <h2 class="text-sm font-medium">{text.empty}</h2>
      <p class="max-w-64 text-[13px] text-muted-foreground">{text.emptyDescription}</p>
    </div>
    {/if}
    <p class="mt-2 text-xs text-muted-foreground">{text.contentCount(data.files.length)}</p>
    {#if data.files.length === 50}<p class="mt-1 text-[13px] text-muted-foreground">{text.libraryLimit}</p>{/if}
  </section>
</div>
