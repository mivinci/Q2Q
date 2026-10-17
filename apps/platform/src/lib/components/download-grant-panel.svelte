<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import { messages } from '#lib/i18n.js';
  import { uploadRequest } from '#lib/upload-client.js';
  import { Button } from '#lib/components/ui/button/index.js';
  let { publicationId, name }: { publicationId: string; name: string } = $props();
  const text = $derived(messages[page.data.locale]);
  let grants = $state<{ id: string; createdAt: number; revokedAt: number | null }[]>([]);
  let busy = $state(false);
  let issue = $state('');
  async function refresh() { grants = await uploadRequest(`/api/publications/${publicationId}/download-grants`, undefined, 'GET'); }
  onMount(() => { refresh().catch(() => { issue = text.projectActionFailed; }); });
  async function exportSeed() {
    if (!window.confirm(text.downloadGrantWarning)) return;
    busy = true; issue = '';
    try {
      const { downloadSeed } = await import('#lib/download-seed.js');
      const response = await fetch(`/api/publications/${publicationId}/seed`);
      if (!response.ok) throw new Error('seed unavailable');
      const bytes = new Uint8Array(await response.arrayBuffer());
      const grant = await uploadRequest<{ sourceUrl: string }>(`/api/publications/${publicationId}/download-grants`);
      await downloadSeed(bytes, grant.sourceUrl, name);
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
  <Button size="sm" variant="outline" disabled={busy} onclick={exportSeed}>{text.exportDownloadSeed}</Button>
  <p class="mt-2 text-xs text-muted-foreground">{text.downloadGrantWarning}</p>
  {#each grants as grant (grant.id)}
    <div class="mt-3 flex flex-wrap items-center justify-between gap-2 text-xs">
      <span class="min-w-0 break-all font-mono text-muted-foreground">{grant.id}</span>
      {#if grant.revokedAt}<span>{text.downloadGrantRevoked}</span>{:else}<Button size="sm" variant="ghost" disabled={busy} onclick={() => revoke(grant.id)}>{text.revokeDownloadGrant}</Button>{/if}
    </div>
  {/each}
  {#if issue}<p role="alert" class="mt-2 text-sm text-destructive">{issue}</p>{/if}
</div>
