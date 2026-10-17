<script lang="ts">
  import { onDestroy } from 'svelte';
  import { invalidateAll } from '$app/navigation';
  import { page } from '$app/state';
  import Plus from '@lucide/svelte/icons/plus';
  import File from '@lucide/svelte/icons/file';
  import X from '@lucide/svelte/icons/x';
  import { Button } from '#lib/components/ui/button/index.js';
  import { messages } from '#lib/i18n.js';
  import { formatBytes, uploadMaxSize, uploadStatusLabels, type UploadSummary } from '#lib/upload.js';
  import { uploadFile, uploadRequest, type UploadProgress } from '#lib/upload-client.js';

  let { projectId, ready, storageEnabled, localUploads, uploads, detail = false, retryFile, onActiveChange }: {
    projectId: string; ready: boolean; storageEnabled: boolean; localUploads: boolean; uploads: UploadSummary[]; detail?: boolean; retryFile?: { id: string; name: string; size: number }; onActiveChange?: (active: boolean) => void;
  } = $props();
  const text = $derived(messages[page.data.locale]);
  const maxSize = $derived(localUploads ? 64 * 1024 * 1024 : uploadMaxSize);
  let input: HTMLInputElement;
  let active = $state<UploadProgress | null>(null);
  let controller: AbortController | null = null;
  let issue = $state<keyof typeof text | null>(null);
  let cancelling = $state(false);
  let actionId = $state<string | null>(null);
  let retryTask = $state<Pick<UploadSummary, 'fileId' | 'name' | 'size'> | null>(null);
  $effect(() => { onActiveChange?.(active !== null); });
  function showError(cause: unknown) {
    const key = cause instanceof Error ? cause.message : '';
    issue = (key.startsWith('upload') || key.startsWith('file')) && key in text ? key as keyof typeof text : 'uploadFailed';
  }
  async function selected(event: Event) {
    const element = event.currentTarget as HTMLInputElement;
    const file = element.files?.[0];
    element.value = '';
    if (!file || active) return;
    const retry = retryTask;
    retryTask = null;
    issue = null;
    if (retry && (file.name.trim() !== retry.name || file.size !== retry.size)) { issue = 'uploadRetryFileMismatch'; return; }
    if (!file.size || file.size > maxSize) { issue = 'uploadInvalid'; return; }
    controller = new AbortController();
    try { await uploadFile(file, projectId, controller.signal, (progress) => { active = progress; }, retry?.fileId); }
    catch (cause) { if (!(cause instanceof DOMException && cause.name === 'AbortError')) showError(cause); }
    finally { active = null; controller = null; cancelling = false; await invalidateAll(); }
  }
  async function taskAction(task: UploadSummary, operation: 'cancel' | 'complete') {
    actionId = task.id;
    issue = null;
    try {
      await uploadRequest(`/api/uploads/${task.id}${operation === 'complete' ? '/complete' : ''}`, operation === 'complete' ? {} : undefined, operation === 'complete' ? 'POST' : 'DELETE');
    } catch (cause) { showError(cause); }
    finally { actionId = null; await invalidateAll(); }
  }
  onDestroy(() => controller?.abort());
</script>

<div class="mb-7">
  <div class="flex flex-wrap items-start justify-between gap-4">
    {#if !detail}<div><h1 class="text-2xl font-medium tracking-tight">{text.library}</h1><p class="mt-1 text-xs text-muted-foreground">{text.description}</p></div>{/if}
    <div class="text-right" class:hidden={detail}>
      <input bind:this={input} type="file" class="hidden" tabindex="-1" aria-label={text.upload} onchange={selected} disabled={!ready || !!active} />
      <Button disabled={!ready || !!active} onclick={() => { retryTask = null; input.click(); }}><Plus class="size-4" aria-hidden="true" />{text.upload}</Button>
      <p class="mt-2 max-w-64 text-xs text-muted-foreground">{ready ? text.uploadLimit(formatBytes(maxSize, page.data.locale)) : storageEnabled ? text.uploadUnavailable : text.uploadStorageRequired}</p>
      {#if !storageEnabled}<a href="/storage" class="mt-1 inline-block text-xs text-primary">{text.storage}</a>{/if}
    </div>
  </div>
  {#if issue}<p role="alert" class="mt-4 text-xs text-destructive">{typeof text[issue] === 'string' ? text[issue] : text.uploadFailed}</p>{/if}
  {#if active}
    <div class="mt-6 rounded-xl border bg-card p-4" role="status">
      <div class="flex items-center gap-3">
        <File class="size-5 shrink-0 text-primary" aria-hidden="true" />
        <div class="min-w-0 flex-1"><p class="truncate text-sm">{active.name}</p><p class="mt-1 text-xs text-muted-foreground">{active.phase === 'initializing' ? text.uploadInitializing : active.phase === 'completing' ? text.uploadCompleting : text.uploading}</p></div>
        <Button variant="ghost" size="icon-sm" aria-label={text.cancelUpload} disabled={active.phase === 'completing' || cancelling} onclick={() => { cancelling = true; controller?.abort(); }}><X class="size-4" aria-hidden="true" /></Button>
      </div>
      <progress class="mt-4 h-1.5 w-full overflow-hidden rounded-full accent-primary [&::-webkit-progress-bar]:bg-muted [&::-webkit-progress-value]:bg-primary [&::-moz-progress-bar]:bg-primary" max={active.size} value={active.sent} aria-label={text.uploadProgress}></progress>
      <p class="mt-2 text-right text-xs tabular-nums text-muted-foreground">{formatBytes(active.sent, page.data.locale)} / {formatBytes(active.size, page.data.locale)}</p>
    </div>
  {/if}
  {#if detail && retryFile && !uploads.length}
    <Button disabled={!ready || !!active} onclick={() => { retryTask = { fileId: retryFile!.id, name: retryFile!.name, size: retryFile!.size }; input.click(); }}>{text.uploadRetry}</Button>
  {/if}
  {#if uploads.length}
    <section aria-label={text.uploadHistory} class="mt-7 overflow-hidden rounded-xl border">
      <div class="border-b bg-muted/30 px-4 py-3"><h2 class="text-xs font-medium">{text.uploadHistory}</h2><p class="mt-1 text-xs text-muted-foreground">{text.uploadHistoryDescription}</p></div>
      <ul class="divide-y">
        {#each uploads as task (task.id)}
          {#if task.id !== active?.id}
            <li class="flex items-center gap-3 px-4 py-3">
              <File class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
              <div class="min-w-0 flex-1"><p class="truncate text-xs">{task.name}</p><p class="mt-1 text-[11px] text-muted-foreground">{formatBytes(task.size, page.data.locale)} · {task.cleanupPending ? text.uploadCleanupPending : text[uploadStatusLabels[task.status]]}</p></div>
              {#if task.canManage && (task.status === 'uploading' || ((task.status === 'cancelled' || task.status === 'failed') && task.cleanupPending))}
                <Button variant="ghost" size="sm" disabled={actionId !== null} onclick={() => taskAction(task, 'cancel')}>{task.cleanupPending ? text.uploadRetryCleanup : text.cancelUpload}</Button>
              {:else if task.canManage && (task.status === 'cancelled' || task.status === 'failed') && !task.cleanupPending}
                <Button variant="outline" size="sm" disabled={!ready || !!active || uploads.some((other) => other.fileId === task.fileId && ['initializing', 'uploading', 'completing', 'uploaded'].includes(other.status))}
                  onclick={() => { retryTask = task; input.click(); }}>{text.uploadRetry}</Button>
              {:else if task.canManage && task.status === 'completing'}
                <Button variant="outline" size="sm" disabled={actionId !== null} onclick={() => taskAction(task, 'complete')}>{text.uploadRetryComplete}</Button>
              {/if}
            </li>
          {/if}
        {/each}
      </ul>
    </section>
  {/if}
</div>
