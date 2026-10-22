<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import { messages } from '#lib/i18n.js';
  import Check from '@lucide/svelte/icons/check';
  import Copy from '@lucide/svelte/icons/copy';
  import { uploadRequest } from '#lib/upload-client.js';
  import { Button } from '#lib/components/ui/button/index.js';
  import ConfirmDialog from '#lib/components/confirm-dialog.svelte';
  let { publicationId, name, infoHash, networkMode, trackUrl }: {
    publicationId: string; name: string; infoHash: string; networkMode: 'platform' | 'custom' | 'unconfigured'; trackUrl: string | null;
  } = $props();
  const text = $derived(messages[page.data.locale]);
  const magnetLink = $derived.by(() => {
    const encode = (value: string) => encodeURIComponent(value).replace(/[!'()*]/g, (char) => `%${char.charCodeAt(0).toString(16).toUpperCase()}`);
    const params = [`xt=urn:sha256:${infoHash}`];
    if (networkMode === 'custom' && trackUrl) params.push(`tr=${encode(trackUrl.replace(/^https:\/\//i, ''))}`);
    return `magnet:?${params.join('&')}`;
  });
  let grants = $state<{ id: string; createdAt: number; revokedAt: number | null }[]>([]);
  let busy = $state(false);
  let issue = $state('');
  let magnetCopied = $state(false);
  let magnetCopyFailed = $state(false);
  let confirmExport = $state(false);
  async function refresh() { grants = await uploadRequest(`/api/publications/${publicationId}/download-grants`, undefined, 'GET'); }
  async function copyMagnet() {
    magnetCopied = false;
    magnetCopyFailed = false;
    try {
      await navigator.clipboard.writeText(magnetLink);
      magnetCopied = true;
    } catch {
      magnetCopyFailed = true;
    }
  }
  onMount(() => { refresh().catch(() => { issue = text.projectActionFailed; }); });
  async function exportSeed() {
    busy = true; issue = '';
    try {
      const { downloadSeed } = await import('#lib/download-seed.js');
      const response = await fetch(`/api/publications/${publicationId}/seed`);
      if (!response.ok) throw new Error('seed unavailable');
      const bytes = new Uint8Array(await response.arrayBuffer());
      const grant = await uploadRequest<{ sourceUrl: string; trackUrl: string | null }>(`/api/publications/${publicationId}/download-grants`);
      await downloadSeed(bytes, grant.sourceUrl, name, grant.trackUrl);
    } catch { issue = text.projectActionFailed; }
    finally { await refresh().catch(() => {}); busy = false; }
  }
  async function revoke(id: string) {
    busy = true; issue = '';
    try { await uploadRequest(`/api/publications/${publicationId}/download-grants/${id}`, undefined, 'DELETE'); await refresh(); }
    catch { issue = text.projectActionFailed; }
    finally { busy = false; }
  }
</script>
<div class="mt-3 w-full rounded-lg bg-muted/40 p-3">
  <Button size="sm" variant="outline" disabled={busy} onclick={() => confirmExport = true}>{text.exportDownloadSeed}</Button>
  <p class="mt-2 text-[13px] text-muted-foreground">{text.downloadGrantWarning}</p>
  <div class="mt-4 border-t pt-3">
    <p class="text-[13px] font-medium">{text.magnetLink}</p>
    <div class="mt-2 flex flex-col gap-2 sm:flex-row">
      <input aria-label={text.magnetLink} readonly value={magnetLink} onfocus={(event) => event.currentTarget.select()} class="h-10 min-w-0 flex-1 rounded-md border bg-background px-3 font-mono text-[13px] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" />
      <Button size="sm" variant="outline" class="h-10" onclick={copyMagnet}>{#if magnetCopied}<Check class="size-4" />{text.magnetCopied}{:else}<Copy class="size-4" />{text.copyMagnetLink}{/if}</Button>
    </div>
    <p class="mt-2 text-[13px] text-muted-foreground">{text.magnetAvailabilityNote}</p>
    {#if magnetCopyFailed}<p role="status" class="mt-2 text-[13px] text-destructive">{text.magnetCopyFailed}</p>{/if}
    {#if magnetCopied}<p role="status" class="sr-only">{text.magnetCopied}</p>{/if}
  </div>
  {#each grants as grant (grant.id)}
    <div class="mt-3 flex flex-wrap items-center justify-between gap-2 text-[13px]">
      <span class="min-w-0 break-all font-mono text-muted-foreground">{grant.id}</span>
      {#if grant.revokedAt}<span>{text.downloadGrantRevoked}</span>{:else}<Button size="sm" variant="ghost" disabled={busy} onclick={() => revoke(grant.id)}>{text.revokeDownloadGrant}</Button>{/if}
    </div>
  {/each}
  {#if issue}<p role="alert" class="mt-2 text-sm text-destructive">{issue}</p>{/if}
</div>

<ConfirmDialog
  open={confirmExport}
  title={text.exportDownloadSeed}
  description={text.downloadGrantWarning}
  confirmLabel={text.exportDownloadSeed}
  cancelLabel={text.cancel}
  destructive={false}
  onOpenChange={(open) => confirmExport = open}
  onConfirm={() => { confirmExport = false; void exportSeed(); }}
/>
