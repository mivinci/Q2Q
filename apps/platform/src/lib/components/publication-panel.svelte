<script lang="ts">
  import { refreshAll } from '$app/navigation';
  import { page } from '$app/state';
  import { messages } from '#lib/i18n.js';
  import { uploadRequest } from '#lib/upload-client.js';
  import type { PublicationSummary } from '#lib/publication.js';
  import { Button } from '@q2q/ui/button';
  import ConfirmDialog from '@q2q/ui/confirm-dialog';
  import DownloadGrantPanel from './download-grant-panel.svelte';
  let { fileId, publications, ready, canManage, networkLabel }: { fileId: string; publications: PublicationSummary[]; ready: boolean; canManage: boolean; networkLabel?: string } = $props();
  const text = $derived(messages[page.data.locale]);
  let busy = $state(false);
  let issue = $state('');
  let requestId = $state<string | null>(null);
  let confirmWithdrawId = $state<string | null>(null);
  const activePublication = $derived(publications.find((version) => version.status === 'creating' || version.status === 'published'));
  const active = $derived(!!activePublication);
  const current = $derived(activePublication ?? publications[0]);
  const history = $derived(publications.filter(version=>version.id!==current?.id));
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

<section class="mb-5 rounded-xl border bg-card p-4 sm:p-5" aria-label={text.publications}>
  <header class="flex flex-wrap items-start justify-between gap-3">
    <div class="min-w-0"><h2 class="text-sm font-medium">{text.publications}</h2><p class="mt-1 text-[13px] leading-relaxed text-muted-foreground">{text.publicationDescription}</p></div>
    {#if canManage&&!active}<Button variant={ready?'default':'outline'} disabled={!ready||busy} onclick={()=>publish()}>{text.publishVersion}</Button>{/if}
  </header>
  {#if networkLabel}<div class="mt-4 flex flex-wrap items-center justify-between gap-2 border-y py-2 text-xs"><p class="text-muted-foreground">{text.nextPublicationNetwork}<span class="ml-2 font-medium text-foreground">{networkLabel}</span></p><Button href="/network" variant="ghost" size="sm">{text.networkConfigure}</Button></div>{/if}
  {#if !active&&!ready}<p class="mt-3 text-[13px] text-muted-foreground">{text.publicationNeedsSeed}</p>{/if}
  {#snippet versionRow(version:PublicationSummary)}
    <div class="py-4">
      <div class="flex flex-wrap items-center justify-between gap-3">
        <div class="min-w-0"><p class="text-[13px] font-medium">{text[({creating:'publicationCreating',published:'publicationPublished',failed:'publicationFailed',withdrawn:'publicationWithdrawn'} as const)[version.status]]}</p><a href={`/seeds/${version.id}`} class="mt-1 block break-all font-mono text-[11px] text-muted-foreground underline-offset-4 hover:text-primary hover:underline">{version.id}</a></div>
        {#if canManage&&(version.status!=='withdrawn'||version.cleanupPending)}
          <div class="flex gap-2">{#if version.status==='creating'}<Button variant="outline" size="sm" disabled={busy} onclick={()=>publish(version.id)}>{text.publicationRedispatch}</Button>{/if}<Button variant="ghost" size="sm" disabled={busy} onclick={()=>withdraw(version.id)}>{version.status==='withdrawn'?text.publicationRetryCleanup:text.withdrawVersion}</Button></div>
        {/if}
      </div>
      {#if version.cleanupPending}<p class="mt-2 text-xs text-muted-foreground">{text.publicationCleanup}</p>{/if}
      {#if canManage&&version.status==='published'}<DownloadGrantPanel publicationId={version.id} name={version.name} infoHash={version.infoHash} networkMode={version.networkMode} trackUrl={version.trackUrl} prominent />{/if}
    </div>
  {/snippet}
  {#if current}{#key current.id}{@render versionRow(current)}{/key}{/if}
  {#if history.length}
    <details class="border-t" open={history.some(version=>version.cleanupPending)}>
      <summary class="cursor-pointer py-3 text-[13px] font-medium text-muted-foreground">{text.publicationHistory} · {history.length}</summary>
      <div class="divide-y">{#each history as version(version.id)}{@render versionRow(version)}{/each}</div>
    </details>
  {/if}
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
