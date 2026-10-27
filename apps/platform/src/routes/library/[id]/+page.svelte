<script lang="ts">
  import { goto, refreshAll } from '$app/navigation';
  import { page } from '$app/state';
  import AlertCircle from '@lucide/svelte/icons/circle-alert';
  import Check from '@lucide/svelte/icons/check';
  import File from '@lucide/svelte/icons/file';
  import { messages } from '#lib/i18n.js';
  import { formatBytes } from '#lib/upload.js';
  import { uploadRequest } from '#lib/upload-client.js';
  import { Button } from '@q2q/ui/button';
  import UploadPanel from '#lib/components/upload-panel.svelte';
  import ConfirmDialog from '@q2q/ui/confirm-dialog';
  import ProcessingPanel from '#lib/components/processing-panel.svelte';
  import PublicationPanel from '#lib/components/publication-panel.svelte';
  import * as Dropdown from '@q2q/ui/dropdown-menu';
  import MoreHorizontal from '@lucide/svelte/icons/ellipsis';
  import Trash from '@lucide/svelte/icons/trash-2';
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
  let confirmDelete = $state(false);
  const blocked = $derived(data.publicationDeletionBlocked || uploading || processing || data.uploads.some((task) => ['initializing', 'uploading', 'completing'].includes(task.status) || task.cleanupPending));
  async function remove() {
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
<div class="mx-auto w-full max-w-4xl px-4 py-6 sm:px-7">
  <header class="mb-6">
    <div class="flex items-start gap-3">
      <span class="flex size-10 shrink-0 items-center justify-center rounded-xl bg-muted"><File class="size-5 text-muted-foreground" aria-hidden="true" /></span>
      <div class="min-w-0 flex-1">
        <h1 class="break-words text-xl font-medium tracking-tight sm:text-2xl">{data.file.name}</h1>
        <span class="mt-2 inline-flex rounded-md border bg-muted/40 px-2 py-0.5 text-xs text-muted-foreground">{data.file.status==='uploaded'?text.uploadStored:data.file.status==='deleting'?text.fileDeleting:text.filePending}</span>
      </div>
      {#if data.canManage}
        <Dropdown.Root>
          <Dropdown.Trigger class="flex size-9 shrink-0 items-center justify-center rounded-lg outline-none hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring" aria-label={text.fileMoreActions}><MoreHorizontal class="size-4" /></Dropdown.Trigger>
          <Dropdown.Content align="end" class="w-56">
            <Dropdown.Item variant="destructive" disabled={deleting||blocked} onSelect={()=>confirmDelete=true}><Trash class="size-4" />{text.deleteFile}</Dropdown.Item>
            {#if blocked}<p class="px-2 py-2 text-xs leading-relaxed text-muted-foreground">{data.publicationDeletionBlocked?text.publicationInUse:text.fileUploadActive}</p>{/if}
          </Dropdown.Content>
        </Dropdown.Root>
      {/if}
    </div>
    <dl class="mt-4 flex flex-wrap gap-x-5 gap-y-2 text-xs text-muted-foreground">
      <div class="flex gap-1.5"><dt>{text.size}</dt><dd class="text-foreground">{formatBytes(data.file.size,page.data.locale)}</dd></div>
      <div class="flex gap-1.5"><dt>{text.fileCreatedAt}</dt><dd>{new Intl.DateTimeFormat(page.data.locale,{dateStyle:'medium',timeStyle:'short'}).format(data.file.createdAt)}</dd></div>
      <div class="flex gap-1.5"><dt>{text.updated}</dt><dd>{new Intl.DateTimeFormat(page.data.locale,{dateStyle:'medium',timeStyle:'short'}).format(data.file.updatedAt)}</dd></div>
    </dl>
  </header>
  <nav aria-label={text.fileFlow} class="mb-6 border-y py-4">
    <ol class="grid gap-4 sm:grid-cols-3 sm:gap-5">
      {#each lifecycle as step,index}
        <li aria-current={step.state==='current'?'step':undefined} class="flex min-w-0 items-start gap-2.5">
          <span class={`flex size-6 shrink-0 items-center justify-center rounded-full border text-[11px] ${step.state==='complete'?'border-primary/20 bg-primary/10 text-primary':step.state==='failed'?'border-destructive/30 text-destructive':step.state==='current'?'border-primary text-primary':'border-border text-muted-foreground'}`}>
            {#if step.state==='complete'}<Check class="size-3.5" aria-hidden="true" />{:else if step.state==='failed'}<AlertCircle class="size-3.5" aria-hidden="true" />{:else}{index+1}{/if}
          </span>
          <div class="min-w-0"><p class="text-[13px] font-medium">{step.title}</p><p class={`mt-0.5 text-xs ${step.state==='failed'?'text-destructive':'text-muted-foreground'}`}>{step.detail}</p></div>
        </li>
      {/each}
    </ol>
  </nav>
  {#if issue}<p role="alert" class="mb-4 text-sm text-destructive">{issue}</p>{/if}
  {#snippet uploadPanel()}
    {#if data.currentProject}
      {#key data.file.id}<UploadPanel detail onActiveChange={(value)=>uploading=value} retryFile={data.canManage&&data.file.status==='pending'?data.file:undefined} projectId={data.currentProject.id} ready={data.uploadReady&&data.file.status!=='deleting'&&!deleting} storageEnabled={data.storageEnabled} localUploads={data.localUploads} maxFileSize={data.maxFileSize} storageRemainingBytes={data.storageRemainingBytes} uploads={data.uploads} />{/key}
    {/if}
  {/snippet}
  {#if data.file.status!=='uploaded'}{@render uploadPanel()}{/if}
  {#if data.file.status==='uploaded'}
    {#key data.file.id}<ProcessingPanel fileId={data.file.id} size={data.file.size} summary={data.processing} available={data.processingAvailable} canManage={data.canManage} onBusyChange={(value)=>processing=value} />{/key}
  {/if}
  {#if data.currentProject}
    <PublicationPanel fileId={data.file.id} publications={data.publications} ready={data.processing?.status==='ready'&&data.processingAvailable&&data.file.status==='uploaded'} canManage={data.canManage} networkLabel={data.network.trackUrl?data.network.mode==='custom'?text.networkCustom:text.networkPlatform:text.networkNotConfigured} />
  {/if}
  {#if data.file.status==='uploaded'}{@render uploadPanel()}{/if}
</div>

<ConfirmDialog
  open={confirmDelete}
  title={text.deleteFile}
  description={text.deleteFileConfirm}
  confirmLabel={text.deleteFile}
  cancelLabel={text.cancel}
  onOpenChange={(open) => confirmDelete = open}
  onConfirm={() => { confirmDelete = false; void remove(); }}
/>
