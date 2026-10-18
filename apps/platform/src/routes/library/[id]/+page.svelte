<script lang="ts">
  import { goto, refreshAll } from '$app/navigation';
  import { page } from '$app/state';
  import AlertCircle from '@lucide/svelte/icons/circle-alert';
  import Check from '@lucide/svelte/icons/check';
  import File from '@lucide/svelte/icons/file';
  import { messages } from '#lib/i18n.js';
  import { formatBytes } from '#lib/upload.js';
  import { uploadRequest } from '#lib/upload-client.js';
  import { Button } from '#lib/components/ui/button/index.js';
  import UploadPanel from '#lib/components/upload-panel.svelte';
  import ProcessingPanel from '#lib/components/processing-panel.svelte';
  import PublicationPanel from '#lib/components/publication-panel.svelte';
  import type { PageProps } from './$types';
  let { data }: PageProps = $props();
  const text = $derived(messages[page.data.locale]);
  const lifecycle = $derived.by(() => {
    const uploaded = data.file.status === 'uploaded';
    const seedReady = data.processing?.status === 'ready';
    const latestPublication = data.publications[0];
    const activePublication = data.publications.find((version) => version.status === 'creating' || version.status === 'published');
    const publicationFailed = !activePublication && latestPublication?.status === 'failed';
    const uploadFailed = data.file.status === 'pending' && data.uploads[0]?.status === 'failed';
    return [
      {
        title: text.fileFlowUpload,
        detail: uploaded ? text.uploadStored : data.file.status === 'deleting' ? text.fileDeleting : uploadFailed ? text.uploadFailed : text.filePending,
        state: uploaded ? 'complete' : uploadFailed ? 'failed' : 'current'
      },
      {
        title: text.fileFlowSeed,
        detail: seedReady ? text.processingReady : data.processing?.status === 'failed' ? text.processingFailed : data.processing?.status === 'running' ? text.processingRunning : text.processingPending,
        state: !uploaded ? 'pending' : seedReady ? 'complete' : data.processing?.status === 'failed' ? 'failed' : 'current'
      },
      {
        title: text.fileFlowPublish,
        detail: activePublication?.status === 'published' ? text.publicationPublished
          : activePublication?.status === 'creating' ? text.publicationCreating
          : publicationFailed ? text.publicationFailed
          : latestPublication?.status === 'withdrawn' ? text.publicationWithdrawn
          : text.fileFlowReadyToPublish,
        state: activePublication?.status === 'published' ? 'complete'
          : !seedReady ? 'pending'
          : publicationFailed ? 'failed'
          : activePublication?.status === 'creating' ? 'current'
          : 'current'
      }
    ] as const;
  });
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
      await refreshAll();
    } finally { deleting = false; }
  }
</script>

<svelte:head><title>{data.file.name} · Q2Q</title></svelte:head>
<div class="mx-auto w-full max-w-4xl px-4 py-8 sm:px-7">
  <div class="mb-5 flex items-center gap-3"><File class="size-7 shrink-0 text-primary" /><h1 class="min-w-0 wrap-break-word text-2xl font-medium tracking-tight">{data.file.name}</h1></div>
  <section class="mb-5 rounded-xl border bg-card p-5" aria-label={text.fileFlow}>
    <h2 class="mb-4 text-sm font-medium">{text.fileFlow}</h2>
    <ol class="grid gap-3 sm:grid-cols-3">
      {#each lifecycle as step, index}
        <li aria-current={step.state === 'current' ? 'step' : undefined} class="flex min-w-0 items-start gap-3 rounded-lg bg-muted/30 p-3">
          <span class={`flex size-7 shrink-0 items-center justify-center rounded-full border text-xs ${step.state === 'complete' ? 'border-primary bg-primary text-primary-foreground' : step.state === 'failed' ? 'border-destructive text-destructive' : step.state === 'current' ? 'border-primary text-primary' : 'border-muted-foreground/30 text-muted-foreground'}`}>
            {#if step.state === 'complete'}<Check class="size-4" />{:else if step.state === 'failed'}<AlertCircle class="size-4" />{:else}{index + 1}{/if}
          </span>
          <div class="min-w-0"><p class="text-[13px] font-medium">{step.title}</p><p class={`mt-1 text-[13px] ${step.state === 'failed' ? 'text-destructive' : 'text-muted-foreground'}`}>{step.detail}</p></div>
        </li>
      {/each}
    </ol>
  </section>
  <dl class="mb-5 grid gap-5 rounded-xl border bg-card p-5 sm:grid-cols-2">
    <div><dt class="text-[13px] text-muted-foreground">{text.size}</dt><dd class="mt-2 text-sm">{formatBytes(data.file.size, page.data.locale)}</dd></div>
    <div><dt class="text-[13px] text-muted-foreground">{text.fileCreatedAt}</dt><dd class="mt-2 text-sm">{new Intl.DateTimeFormat(page.data.locale, { dateStyle: 'medium', timeStyle: 'short' }).format(data.file.createdAt)}</dd></div>
    <div><dt class="text-[13px] text-muted-foreground">{text.updated}</dt><dd class="mt-2 text-sm">{new Intl.DateTimeFormat(page.data.locale, { dateStyle: 'medium', timeStyle: 'short' }).format(data.file.updatedAt)}</dd></div>
  </dl>
  {#if data.file.status === 'uploaded'}
    {#key data.file.id}<ProcessingPanel fileId={data.file.id} size={data.file.size} summary={data.processing} available={data.processingAvailable} canManage={data.canManage} onBusyChange={(value) => processing = value} />{/key}
  {/if}
  {#if data.currentProject}
  <div class="mb-5 flex flex-wrap items-center justify-between gap-3 rounded-xl border bg-card p-4 text-sm"><div><p class="text-[13px] text-muted-foreground">{text.nextPublicationNetwork}</p><p class="mt-1">{data.network.trackUrl ? data.network.mode === 'custom' ? text.networkCustom : text.networkPlatform : text.networkNotConfigured}</p></div><Button href="/network" variant="ghost" size="sm">{text.networkConfigure}</Button></div>
    <PublicationPanel fileId={data.file.id} publications={data.publications} ready={data.processing?.status === 'ready' && data.processingAvailable && data.file.status === 'uploaded'} canManage={data.canManage} />
    {#key data.file.id}
      <UploadPanel detail onActiveChange={(value) => uploading = value} retryFile={data.canManage && data.file.status === 'pending' ? data.file : undefined} projectId={data.currentProject.id} ready={data.uploadReady && data.file.status !== 'deleting' && !deleting} storageEnabled={data.storageEnabled} localUploads={data.localUploads} maxFileSize={data.maxFileSize} uploads={data.uploads} />
    {/key}
  {/if}
  {#if issue}<p role="alert" class="mb-4 text-sm text-destructive">{issue}</p>{/if}
  {#if data.canManage}<div class="border-t pt-5"><Button variant="destructive" disabled={deleting || blocked} onclick={remove}>{text.deleteFile}</Button>{#if blocked}<p class="mt-2 text-[13px] text-muted-foreground">{data.publicationDeletionBlocked ? text.publicationInUse : text.fileUploadActive}</p>{/if}</div>{/if}
</div>
