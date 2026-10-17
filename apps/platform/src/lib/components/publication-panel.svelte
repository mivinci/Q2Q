<script lang="ts">
  import { invalidateAll } from '$app/navigation';
  import { page } from '$app/state';
  import { messages } from '#lib/i18n.js';
  import { uploadRequest } from '#lib/upload-client.js';
  import type { PublicationSummary } from '#lib/publication.js';
  import { Button } from '#lib/components/ui/button/index.js';
  import DownloadGrantPanel from './download-grant-panel.svelte';
  let { fileId, publications, ready, canManage }: { fileId: string; publications: PublicationSummary[]; ready: boolean; canManage: boolean } = $props();
  const text = $derived(messages[page.data.locale]);
  let busy = $state(false);
  let issue = $state('');
  let requestId = $state<string | null>(null);
  const active = $derived(publications.some((version) => version.status === 'creating' || version.status === 'published'));
  $effect(() => {
    if (!publications.some((version) => version.status === 'creating' || (version.status === 'withdrawn' && version.cleanupPending))) return;
    const timer = setInterval(() => { void invalidateAll(); }, 2500);
    return () => clearInterval(timer);
  });
  async function publish(id?: string) {
    busy = true; issue = '';
    requestId ??= crypto.randomUUID();
    try {
      await uploadRequest(`/api/files/${fileId}/publications`, { id: id ?? requestId });
      requestId = null;
      await invalidateAll();
    } catch { issue = text.projectActionFailed; }
    finally { busy = false; }
  }
  async function withdraw(id: string) {
    if (publications.find((version) => version.id === id)?.status !== 'withdrawn' && !window.confirm(text.publicationWithdrawConfirm)) return;
    busy = true; issue = '';
    try { await uploadRequest(`/api/publications/${id}`, undefined, 'DELETE'); await invalidateAll(); }
    catch { issue = text.projectActionFailed; }
    finally { busy = false; }
  }
</script>

<section class="mb-7 rounded-xl border bg-card p-5" aria-label={text.publications}>
  <div class="flex flex-wrap items-start justify-between gap-4">
    <div><h2 class="text-sm font-medium">{text.publications}</h2><p class="mt-2 text-xs text-muted-foreground">{text.publicationDescription}</p></div>
    {#if canManage}<Button size="sm" disabled={!ready || active || busy} onclick={() => publish()}>{text.publishVersion}</Button>{/if}
  </div>
  {#each publications as version (version.id)}
    <div class="mt-4 flex flex-wrap items-center justify-between gap-3 border-t pt-4">
      <div class="min-w-0"><p class="text-sm">{text[({ creating: 'publicationCreating', published: 'publicationPublished', failed: 'publicationFailed', withdrawn: 'publicationWithdrawn' } as const)[version.status]]}</p><p class="mt-1 break-all font-mono text-xs text-muted-foreground">{version.id}</p></div>
      {#if version.cleanupPending}<p class="text-xs text-muted-foreground">{text.publicationCleanup}</p>{/if}
      {#if canManage && version.status === 'published'}<DownloadGrantPanel publicationId={version.id} name={version.name} />{/if}
      {#if canManage && (version.status !== 'withdrawn' || version.cleanupPending)}
        <div class="flex gap-2">{#if version.status === 'creating'}<Button variant="ghost" size="sm" disabled={busy} onclick={() => publish(version.id)}>{text.publicationRedispatch}</Button>{/if}<Button variant="outline" size="sm" disabled={busy} onclick={() => withdraw(version.id)}>{version.status === 'withdrawn' ? text.publicationRetryCleanup : text.withdrawVersion}</Button></div>
      {/if}
    </div>
  {/each}
  {#if issue}<p role="alert" class="mt-3 text-sm text-destructive">{issue}</p>{/if}
</section>
