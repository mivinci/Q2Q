<script lang="ts">
  import Sun from '@lucide/svelte/icons/sun';
  import Moon from '@lucide/svelte/icons/moon';
  import Monitor from '@lucide/svelte/icons/monitor';
  import { setMode, userPrefersMode } from 'mode-watcher';
  import { Button } from '#lib/components/ui/button/index.js';

  import { page } from '$app/state';
  import { messages } from '#lib/i18n.js';
  const text = $derived(messages[page.data.locale]);

  let { showLabels = false }: { showLabels?: boolean } = $props();

  const modes = [
    { value: 'light', label: 'light', icon: Sun },
    { value: 'dark', label: 'dark', icon: Moon },
    { value: 'system', label: 'system', icon: Monitor }
  ] as const;
</script>

<div role="group" aria-label={text.appearanceSettings} class="flex rounded-lg bg-border/60 p-0.5">
  {#each modes as mode}
    <Button
      variant="ghost"
      size={showLabels ? "sm" : "icon-sm"}
      class={userPrefersMode.current === mode.value
        ? 'flex-1 bg-background text-foreground shadow-sm hover:bg-background'
        : 'flex-1 text-muted-foreground'}
      aria-label={text[mode.label]}
      aria-pressed={userPrefersMode.current === mode.value}
      onclick={() => setMode(mode.value)}
    >
      <mode.icon class="size-3.5" aria-hidden="true" />
      {#if showLabels}<span>{text[mode.label]}</span>{/if}
    </Button>
  {/each}
</div>
