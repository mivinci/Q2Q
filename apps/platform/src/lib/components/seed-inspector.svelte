<script lang="ts">
  import { Dialog } from 'bits-ui';
  import { page } from '$app/state';
  import FileSearch from '@lucide/svelte/icons/file-search';
  import { messages } from '#lib/i18n.js';
  import { formatBytes } from '#lib/upload.js';
  import { Button } from '#lib/components/ui/button/index.js';
  import type { PublicationSummary } from '#lib/publication.js';
  import type { DecodedSeed } from '#lib/seed-codec.js';
  let { seeds }: { seeds: PublicationSummary[] } = $props();
  const text = $derived(messages[page.data.locale]);
  let open = $state(false);
  let busy = $state(false);
  let issue = $state(false);
  let decoded = $state<DecodedSeed | null>(null);
  let chooser: HTMLInputElement;
  let generation = 0;
  const matches = $derived(decoded ? seeds.filter((seed) => seed.infoHash === decoded!.infoHash) : []);
  function close() { open = false; generation++; decoded = null; issue = false; busy = false; }
  async function parse(file: File | undefined) {
    if (!file) return;
    const attempt = ++generation;
    decoded = null; issue = false; busy = true;
    try {
      if (!file.size || file.size > 2 * 1024 * 1024) throw new Error('size');
      const { inspectSeed } = await import('#lib/seed-codec.js');
      const result = await inspectSeed(new Uint8Array(await file.arrayBuffer()));
      if (attempt === generation && open) decoded = result;
    } catch { if (attempt === generation && open) issue = true; }
    finally { if (attempt === generation) busy = false; }
  }
  function drop(event: DragEvent) { event.preventDefault(); void parse(event.dataTransfer?.files[0]); }
</script>

<Button variant="outline" onclick={() => open = true}><FileSearch class="size-4" />{text.inspectSeed}</Button>
<Dialog.Root {open} onOpenChange={(value) => { if (!value) close(); else open = true; }}>
  <Dialog.Portal>
    <Dialog.Overlay class="fixed inset-0 z-[60] bg-black/35 backdrop-blur-sm" />
    <Dialog.Content class="fixed top-1/2 left-1/2 z-[60] max-h-[calc(100dvh-2rem)] w-[calc(100%-2rem)] max-w-xl -translate-x-1/2 -translate-y-1/2 overflow-y-auto rounded-2xl border bg-popover p-6 text-popover-foreground shadow-xl outline-none">
      <Dialog.Title class="text-lg font-semibold tracking-tight">{text.inspectSeed}</Dialog.Title>
      <Dialog.Description class="mt-2 text-sm text-muted-foreground">{text.seedLocalDescription}</Dialog.Description>
      <div role="region" aria-label={text.seedChoose} ondragover={(event) => event.preventDefault()} ondrop={drop} class="my-5 rounded-xl border border-dashed bg-muted/30 p-5 text-center">
        <input bind:this={chooser} type="file" accept=".q2q" class="hidden" aria-label={text.seedChoose} onchange={(event) => { void parse(event.currentTarget.files?.[0]); event.currentTarget.value = ''; }} />
        <Button variant="ghost" onclick={() => chooser.click()}>{text.seedChoose}</Button>
        <p class="mt-2 text-[13px] text-muted-foreground">{text.seedLimit}</p>
      </div>
      {#if busy}<p role="status" class="text-sm">{text.seedInspecting}</p>{/if}
      {#if issue}<p role="alert" class="text-sm text-destructive">{text.seedInvalid}</p>{/if}
      {#if decoded}
        <p role="status" class="mb-4 text-sm text-primary">{text.seedValid}</p>
        <dl class="grid gap-4 text-sm sm:grid-cols-2">
          <div class="sm:col-span-2"><dt class="text-[13px] text-muted-foreground">{text.name}</dt><dd class="mt-1 break-all">{decoded.name ?? text.seedUnnamed}</dd></div>
          <div><dt class="text-[13px] text-muted-foreground">{text.size}</dt><dd class="mt-1">{Number.isSafeInteger(Number(decoded.size)) ? formatBytes(Number(decoded.size), page.data.locale) : decoded.size}</dd></div>
          <div><dt class="text-[13px] text-muted-foreground">{text.seedBlockSize}</dt><dd class="mt-1">{formatBytes(decoded.blockSize, page.data.locale)}</dd></div>
          <div><dt class="text-[13px] text-muted-foreground">{text.seedBlockCount}</dt><dd class="mt-1">{decoded.blockCount}</dd></div>
          <div><dt class="text-[13px] text-muted-foreground">{text.seedProof}</dt><dd class="mt-1">{decoded.inlineProof ? text.seedInlineProof : text.seedExternalProof}</dd></div>
          <div class="sm:col-span-2"><dt class="text-[13px] text-muted-foreground">{text.seedContentIdentity}</dt><dd class="mt-1 break-all font-mono text-xs">{decoded.infoHash}</dd></div>
          <div class="sm:col-span-2"><dt class="text-[13px] text-muted-foreground">{text.seedRoot}</dt><dd class="mt-1 break-all font-mono text-xs">{decoded.root}</dd></div>
          {#each [{ label: text.seedTracks, values: decoded.tracks, empty: text.networkNotConfigured }, { label: text.seedSources, values: decoded.sources, empty: text.seedNoSources }, { label: text.seedProofSources, values: decoded.proofSources, empty: decoded.inlineProof ? text.seedProofEmbedded : text.seedNoProofSources }] as group}
            <div class="sm:col-span-2"><dt class="text-[13px] text-muted-foreground">{group.label}</dt><dd class="mt-1 break-all text-[13px]">{group.values.length ? group.values.join(' · ') : group.empty}</dd></div>
          {/each}
        </dl>
        {#if !decoded.tracks.length && !decoded.sources.length}<p class="mt-4 text-[13px] text-muted-foreground">{text.seedNoDownloadHint}</p>{/if}
        <div class="mt-5 border-t pt-4"><p class="text-[13px] text-muted-foreground">{matches.length ? text.seedMatches : text.seedNoMatch}</p>{#each matches as seed}<a href={`/seeds/${seed.id}`} onclick={close} class="mt-2 block break-all text-sm text-primary hover:underline">{text.seedViewDetails} · {seed.id}</a>{/each}</div>
      {/if}
      <div class="mt-6 flex justify-end"><Button variant="outline" onclick={close}>{text.cancel}</Button></div>
    </Dialog.Content>
  </Dialog.Portal>
</Dialog.Root>
