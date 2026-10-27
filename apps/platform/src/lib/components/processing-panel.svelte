<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import Download from '@lucide/svelte/icons/download';
  import { messages } from '#lib/i18n.js';
  import { Button } from '@q2q/ui/button';
  import { uploadRequest } from '#lib/upload-client.js';
  import { formatBytes } from '#lib/upload.js';
  import type { ProcessingSummary } from '#lib/processing.js';
  let { fileId, size, summary, canManage, available, onBusyChange }: {
    fileId: string; size: number; summary: ProcessingSummary | null; canManage: boolean; available: boolean; onBusyChange: (busy: boolean) => void;
  } = $props();
  const text = $derived(messages[page.data.locale]);
  let polled = $state<ProcessingSummary | null | undefined>();
  const task = $derived(polled === undefined ? summary : polled);
  let sending = $state(false);
  let issue = $state(false);
  const busy = $derived(task?.status === 'pending' || task?.status === 'running');
  const retryable = $derived(!task || task.status === 'failed' || (busy && Date.now() - task.updatedAt > 120_000));
  const labels = { pending: 'processingPending', running: 'processingRunning', ready: 'processingReady', failed: 'processingFailed' } as const;
  $effect(() => { summary; polled = undefined; });
  $effect(() => { onBusyChange(busy || sending); });
  async function start() {
    sending = true;
    issue = false;
    try { polled = await uploadRequest<ProcessingSummary>(`/api/files/${fileId}/processing`, {}); }
    catch { issue = true; }
    finally { sending = false; }
  }
  onMount(() => {
    let stopped = false;
    let timer: ReturnType<typeof setTimeout>;
    const controller = new AbortController();
    async function poll() {
      if (stopped) return;
      if (busy || sending) {
        try { polled = await uploadRequest<ProcessingSummary | null>(`/api/files/${fileId}/processing`, undefined, 'GET', controller.signal); }
        catch { if (!stopped) issue = true; }
      }
      if (!stopped) timer = setTimeout(poll, 2000);
    }
    timer = setTimeout(poll, 2000);
    return () => { stopped = true; clearTimeout(timer); controller.abort(); };
  });
</script>

<section class="mb-7 rounded-xl border bg-card p-5" aria-label={text.processingTitle}>
  <div class="flex flex-wrap items-center justify-between gap-3">
    <div><h2 class="text-sm font-medium">{text.processingTitle}</h2><p class="mt-2 text-[13px] text-muted-foreground" role="status">{task ? text[labels[task.status]] : text.uploadAwaitingProcessing}</p></div>
    {#if task?.status === 'ready'}
      <Button href={`/api/files/${fileId}/seed`} variant="outline" size="sm"><Download class="size-4" />{text.downloadSeed}</Button>
    {:else if canManage && retryable}
      <Button variant="outline" size="sm" disabled={sending || !available} onclick={start}>{task ? text.processingRetry : text.processingStart}</Button>
    {/if}
  </div>
  {#if task?.status === 'running'}
    <progress class="mt-4 h-1.5 w-full accent-primary" max={size} value={task.processedBytes} aria-label={text.processingProgress}></progress>
    <p class="mt-2 text-xs tabular-nums text-muted-foreground">{formatBytes(task.processedBytes, page.data.locale)} / {formatBytes(size, page.data.locale)}</p>
  {/if}
  {#if task?.infoHash}<p class="mt-4 break-all font-mono text-[11px] text-muted-foreground">{text.contentIdentity}: {task.infoHash}</p>{/if}
  {#if issue || task?.status === 'failed'}<p role="alert" class="mt-3 text-[13px] text-destructive">{task?.errorCode === 'processingSourceChanged' ? text.processingSourceChanged : issue || task?.errorCode === 'processingUnavailable' ? text.processingUnavailable : text.processingFailed}</p>{/if}
  {#if !available && task?.status !== 'ready'}<p class="mt-3 text-[13px] text-muted-foreground">{text.processingUnavailable}</p>{/if}
  <p class="mt-3 text-[13px] text-muted-foreground">{text.processingDescription}</p>
</section>
