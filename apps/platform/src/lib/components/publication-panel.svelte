<script lang="ts">
  import { refreshAll } from '$app/navigation';
  import { page } from '$app/state';
  import { messages } from '#lib/i18n.js';
  import { uploadRequest } from '#lib/upload-client.js';
  import type { PublicationSummary } from '#lib/publication.js';
  import { Button } from '#lib/components/ui/button/index.js';
  import ConfirmDialog from '#lib/components/confirm-dialog.svelte';
  import DownloadGrantPanel from './download-grant-panel.svelte';
  let { fileId, publications, ready, canManage }: { fileId: string; publications: PublicationSummary[]; ready: boolean; canManage: boolean } = $props();
  const text = $derived(messages[page.data.locale]);
  let busy = $state(false);
  let issue = $state('');
  let requestId = $state<string | null>(null);
  let confirmWithdrawId = $state<string | null>(null);
  const activePublication = $derived(publications.find((version) => version.status === 'creating' || version.status === 'published'));
  const active = $derived(!!activePublication);
  $effect(() => {
    if (!publications.some((version) => version.status === 'creating' || (version.status === 'withdrawn' && version.cleanupPending))) return;
    const timer = setInterval(() => { void refreshAll(); }, 2500);
    return () => clearInterval(timer);
  });
  async function publish(id?: string) {
    busy = true; issue = '';
    requestId ??= crypto.randomUUID();
    try {
      await uploadRequest(`/api/files/${fileId}/publications`, { id: id ?? requestId });
      requestId = null;
      await refreshAll();
    } catch { issue = text.projectActionFailed; }
    finally { busy = false; }
  }
  async function withdraw(id: string) {
    if (publications.find((version) => version.id === id)?.status !== 'withdrawn') {
      confirmWithdrawId = id;
      return;
    }
    await withdrawConfirmed(id);
  }
  async function withdrawConfirmed(id: string) {
    busy = true; issue = '';
    try { await uploadRequest(`/api/publications/${id}`, undefined, 'DELETE'); await refreshAll(); }
    catch { issue = text.projectActionFailed; }
    finally { busy = false; }
  }
</script>

<section class="mb-7 rounded-xl border bg-card p-5" aria-label={text.publications}>
  <div class="grid gap-4 sm:grid-cols-[minmax(0,1fr)_auto] sm:items-start">
    <div class="min-w-0"><h2 class="text-sm font-medium">{text.publications}</h2><p class="mt-2 text-[13px] text-muted-foreground">{text.publicationDescription}</p></div>
    {#if canManage}
      <div class="flex max-w-56 flex-col items-start gap-1 sm:items-end">
        <Button size="sm" disabled={!ready || active || busy} onclick={() => publish()}>{text.publishVersion}</Button>
        {#if activePublication?.status === 'creating'}<p class="text-[13px] text-muted-foreground sm:text-right">{text.publicationWaitActive}</p>
        {:else if activePublication?.status === 'published'}<p class="text-[13px] text-muted-foreground sm:text-right">{text.publicationWithdrawForNew}</p>
        {:else if !ready}<p class="text-[13px] text-muted-foreground sm:text-right">{text.publicationNeedsSeed}</p>{/if}
      </div>
    {/if}
  </div>
  {#each publications as version (version.id)}
    <div class="mt-4 flex flex-wrap items-center justify-between gap-3 border-t pt-4">
      <div class="min-w-0"><p class="text-sm">{text[({ creating: 'publicationCreating', published: 'publicationPublished', failed: 'publicationFailed', withdrawn: 'publicationWithdrawn' } as const)[version.status]]}</p><a href={`/seeds/${version.id}`} class="mt-1 block break-all font-mono text-xs text-primary underline-offset-4 hover:underline">{version.id}</a></div>
      {#if version.cleanupPending}<p class="text-[13px] text-muted-foreground">{text.publicationCleanup}</p>{/if}
      {#if canManage && version.status === 'published'}<DownloadGrantPanel publicationId={version.id} name={version.name} infoHash={version.infoHash} networkMode={version.networkMode} trackUrl={version.trackUrl} />{/if}
      {#if canManage && (version.status !== 'withdrawn' || version.cleanupPending)}
        <div class="flex gap-2">{#if version.status === 'creating'}<Button variant="ghost" size="sm" disabled={busy} onclick={() => publish(version.id)}>{text.publicationRedispatch}</Button>{/if}<Button variant="outline" size="sm" disabled={busy} onclick={() => withdraw(version.id)}>{version.status === 'withdrawn' ? text.publicationRetryCleanup : text.withdrawVersion}</Button></div>
      {/if}
    </div>
  {/each}
  {#if issue}<p role="alert" class="mt-3 text-sm text-destructive">{issue}</p>{/if}
</section>

<ConfirmDialog
  open={confirmWithdrawId !== null}
  title={text.withdrawVersion}
  description={text.publicationWithdrawConfirm}
  confirmLabel={text.withdrawVersion}
  cancelLabel={text.cancel}
  onOpenChange={(open) => { if (!open) confirmWithdrawId = null; }}
  onConfirm={() => { const id = confirmWithdrawId; confirmWithdrawId = null; if (id) void withdrawConfirmed(id); }}
/>
