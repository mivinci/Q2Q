<script lang="ts">
  import { goto, invalidateAll } from '$app/navigation';
  import { page } from '$app/state';
  import ArrowLeft from '@lucide/svelte/icons/arrow-left';
  import File from '@lucide/svelte/icons/file';
  import { messages } from '#lib/i18n.js';
  import { fileStatusLabel, formatBytes } from '#lib/upload.js';
  import { uploadRequest } from '#lib/upload-client.js';
  import { Button } from '#lib/components/ui/button/index.js';
  import UploadPanel from '#lib/components/upload-panel.svelte';
  import ProcessingPanel from '#lib/components/processing-panel.svelte';
  import PublicationPanel from '#lib/components/publication-panel.svelte';
  import type { PageProps } from './$types';
  let { data }: PageProps = $props();
  const text = $derived(messages[page.data.locale]);
  let deleting = $state(false);
  let issue = $state('');
  let uploading = $state(false);
  let processing = $state(false);
  const blocked = $derived(data.publicationDeletionBlocked || uploading || processing || data.uploads.some((task) => ['initializing', 'uploading', 'completing'].includes(task.status) || task.cleanupPending));
  async function remove() {
    if (!window.confirm(text.deleteFileConfirm)) return;
    deleting = true;
    issue = '';
    try {
      await uploadRequest(`/api/files/${data.file.id}`, undefined, 'DELETE');
      await goto('/library', { invalidateAll: true });
    } catch (cause) {
      const key = cause instanceof Error ? cause.message : '';
      issue = key === 'fileUploadActive' ? text.fileUploadActive : key === 'publicationInUse' ? text.publicationInUse : text.projectActionFailed;
      await invalidateAll();
    } finally { deleting = false; }
  }
</script>

<svelte:head><title>{data.file.name} · Q2Q</title></svelte:head>
<div class="mx-auto max-w-4xl px-4 py-8 sm:px-7">
  <Button href="/library" variant="ghost" size="sm" class="mb-6 -ml-3"><ArrowLeft class="size-4" />{text.backToLibrary}</Button>
  <div class="mb-7 flex items-start gap-3"><File class="mt-1 size-7 shrink-0 text-primary" /><div class="min-w-0"><p class="mb-1 text-xs text-muted-foreground">{text.fileDetails}</p><h1 class="break-words text-2xl font-medium tracking-tight">{data.file.name}</h1></div></div>
  <dl class="mb-7 grid gap-5 rounded-xl border bg-card p-5 sm:grid-cols-2">
    <div><dt class="text-xs text-muted-foreground">{text.size}</dt><dd class="mt-2 text-sm">{formatBytes(data.file.size, page.data.locale)}</dd></div>
    <div><dt class="text-xs text-muted-foreground">{text.fileStatus}</dt><dd class="mt-2 text-sm">{data.file.status === 'uploaded' ? text.uploadStored : text[fileStatusLabel(data.file.status, data.uploads[0]?.status)]}</dd></div>
    <div><dt class="text-xs text-muted-foreground">{text.fileCreatedAt}</dt><dd class="mt-2 text-sm">{new Intl.DateTimeFormat(page.data.locale, { dateStyle: 'medium', timeStyle: 'short' }).format(data.file.createdAt)}</dd></div>
    <div><dt class="text-xs text-muted-foreground">{text.updated}</dt><dd class="mt-2 text-sm">{new Intl.DateTimeFormat(page.data.locale, { dateStyle: 'medium', timeStyle: 'short' }).format(data.file.updatedAt)}</dd></div>
  </dl>
  {#if data.file.status === 'uploaded'}
    {#key data.file.id}<ProcessingPanel fileId={data.file.id} size={data.file.size} summary={data.processing} available={data.processingAvailable} canManage={data.canManage} onBusyChange={(value) => processing = value} />{/key}
  {/if}
  {#if data.currentProject}
    <div class="mb-5 flex flex-wrap items-center justify-between gap-3 rounded-xl border bg-card p-4 text-sm"><div><p class="text-xs text-muted-foreground">{text.network}</p><p class="mt-1">{data.network.trackUrl ? data.network.mode === 'custom' ? text.networkCustom : text.networkPlatform : text.networkNotConfigured}</p></div><Button href="/network" variant="ghost" size="sm">{text.networkConfigure}</Button></div>
    <PublicationPanel fileId={data.file.id} publications={data.publications} ready={data.processing?.status === 'ready' && data.processingAvailable && data.file.status === 'uploaded'} canManage={data.canManage} />
    {#key data.file.id}
      <UploadPanel detail onActiveChange={(value) => uploading = value} retryFile={data.canManage && data.file.status === 'pending' ? data.file : undefined} projectId={data.currentProject.id} ready={data.uploadReady && data.file.status !== 'deleting' && !deleting} storageEnabled={data.storageEnabled} localUploads={data.localUploads} uploads={data.uploads} />
    {/key}
  {/if}
  {#if issue}<p role="alert" class="mb-4 text-sm text-destructive">{issue}</p>{/if}
  {#if data.canManage}<div class="border-t pt-5"><Button variant="destructive" disabled={deleting || blocked} onclick={remove}>{text.deleteFile}</Button>{#if blocked}<p class="mt-2 text-xs text-muted-foreground">{data.publicationDeletionBlocked ? text.publicationInUse : text.fileUploadActive}</p>{/if}</div>{/if}
</div>
