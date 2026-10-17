<script lang="ts">
  import { page } from '$app/state';
  import { enhance } from '$app/forms';
  import { messages } from '#lib/i18n.js';
  import { Button } from '#lib/components/ui/button/index.js';
  import type { PageProps } from './$types';
  let { data, form }: PageProps = $props();
  const text = $derived(messages[page.data.locale]);
  let mode = $state<'platform' | 'custom'>('platform');
  let track = $state('');
  let checking = $state(false);
  let saving = $state(false);
  let checkedUrl = $state('');
  let checkState = $state<'ready' | 'failed' | null>(null);
  const owner = $derived(data.currentProject?.role === 'owner');
  $effect(() => { mode = data.network.mode; track = data.network.mode === 'custom' ? data.network.trackUrl ?? '' : ''; checkState = null; });
  async function check() {
    const candidate = mode === 'custom' ? track : data.network.platformUrl;
    if (!candidate || checking) return;
    checking = true; checkState = null; checkedUrl = candidate;
    try {
      const url = new URL(candidate);
      if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.search || url.hash) throw new Error('invalid');
      url.pathname = url.pathname.replace(/\/$/, '') + '/health';
      const response = await fetch(url, { credentials: 'omit', mode: 'cors', redirect: 'error', referrerPolicy: 'no-referrer', cache: 'no-store', signal: AbortSignal.timeout(8000) });
      if (!response.ok || !response.body) throw new Error('unavailable');
      const reader = response.body.getReader();
      let body = '', length = 0;
      const decoder = new TextDecoder();
      try { while (true) { const { done, value } = await reader.read(); if (done) break; length += value.length; if (length > 16384) throw new Error('size'); body += decoder.decode(value, { stream: true }); } }
      finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
      if (JSON.parse(body).service !== 'q2q-track') throw new Error('service');
      checkState = 'ready';
    } catch { checkState = 'failed'; }
    finally { checking = false; }
  }
</script>
<svelte:head><title>{text.network} · Q2Q</title></svelte:head>
<div class="mx-auto max-w-3xl px-4 py-8 sm:px-7">
  <h1 class="text-2xl font-medium tracking-tight">{text.network}</h1><p class="mt-2 text-sm text-muted-foreground">{text.networkDescription}</p>
  <form action="?/save" method="POST" class="mt-7 space-y-4" use:enhance={() => { saving = true; return async ({ update }) => { await update(); saving = false; }; }}>
    <input type="hidden" name="projectId" value={data.currentProject?.id} />
    {#each [{ value: 'platform', title: text.networkPlatform, description: text.networkPlatformDescription }, { value: 'custom', title: text.networkCustom, description: text.networkCustomDescription }] as option}
      <label class={`flex cursor-pointer items-start gap-3 rounded-xl border bg-card p-5 transition-colors ${mode === option.value ? 'border-primary/60 ring-1 ring-primary/10' : ''}`}><input type="radio" name="mode" value={option.value} bind:group={mode} disabled={!owner || saving} class="mt-1 accent-primary" /><span><span class="block text-sm font-medium">{option.title}</span><span class="mt-1 block text-xs leading-relaxed text-muted-foreground">{option.description}</span></span></label>
    {/each}
    <div class="rounded-xl border bg-card p-5">
      {#if mode === 'platform'}
        <p class="text-sm">{data.network.platformUrl ?? text.networkUnavailable}</p><p class="mt-2 text-xs text-muted-foreground">{text.networkPlanPending}</p>
      {:else}
        <label for="track-url" class="mb-2 block text-xs text-muted-foreground">{text.networkTrack}</label><input id="track-url" name="trackUrl" type="url" bind:value={track} required disabled={!owner || saving} placeholder="https://track.example.com" class="h-11 w-full rounded-lg border bg-background px-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring" />
      {/if}
      <Button type="button" variant="outline" size="sm" class="mt-4" disabled={checking || !(mode === 'custom' ? track : data.network.platformUrl)} onclick={check}>{checking ? text.networkChecking : text.networkCheck}</Button>
      {#if checkState && checkedUrl === (mode === 'custom' ? track : data.network.platformUrl)}<p role="status" class={`mt-3 text-xs ${checkState === 'ready' ? 'text-primary' : 'text-muted-foreground'}`}>{checkState === 'ready' ? text.networkReachable : text.networkCheckFailed}</p>{/if}
    </div>
    <p class="text-xs text-muted-foreground">{text.networkFuture}</p>
    {#if form?.error}<p role="alert" class="text-sm text-destructive">{text.networkInvalid}</p>{/if}
    {#if form?.saved}<p role="status" class="text-sm text-primary">{text.networkSaved}</p>{/if}
    {#if owner}<Button type="submit" disabled={saving}>{text.networkSave}</Button>{/if}
  </form>
</div>
