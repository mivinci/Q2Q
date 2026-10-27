<script lang="ts">
  import Sun from '@lucide/svelte/icons/sun';
  import Moon from '@lucide/svelte/icons/moon';
  import Monitor from '@lucide/svelte/icons/monitor';
  import { setMode, userPrefersMode } from 'mode-watcher';
  import { Button } from './button/index.js';

  let { showLabels = false, labels }: {
    showLabels?: boolean;
    labels: { appearanceSettings: string; light: string; dark: string; system: string };
  } = $props();

  const modes = [
    { value: 'light', label: 'light', icon: Sun },
    { value: 'dark', label: 'dark', icon: Moon },
    { value: 'system', label: 'system', icon: Monitor }
  ] as const;
</script>

<div
  role="group"
  aria-label={labels.appearanceSettings}
  class={`flex items-center rounded-lg bg-border/60 ${showLabels ? 'p-0.5' : 'h-10 gap-0.5 p-1'}`}
>
  {#each modes as mode}
    <Button
      variant="ghost"
      size={showLabels ? "sm" : "icon-sm"}
      class={`${showLabels ? 'flex-1' : 'flex-none'} ${userPrefersMode.current === mode.value
        ? 'bg-background text-foreground shadow-sm hover:bg-background'
        : 'text-muted-foreground'}`}
      aria-label={labels[mode.label]}
      aria-pressed={userPrefersMode.current === mode.value}
      onclick={() => setMode(mode.value)}
    >
      <mode.icon class="size-3.5" aria-hidden="true" />
      {#if showLabels}<span>{labels[mode.label]}</span>{/if}
    </Button>
  {/each}
</div>
