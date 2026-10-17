<script lang="ts">
  import Sun from '@lucide/svelte/icons/sun';
  import Moon from '@lucide/svelte/icons/moon';
  import Monitor from '@lucide/svelte/icons/monitor';
  import { setMode, userPrefersMode } from 'mode-watcher';
  import { Button } from '#lib/components/ui/button/index.js';

  const modes = [
    { value: 'light', label: '浅色', icon: Sun },
    { value: 'dark', label: '深色', icon: Moon },
    { value: 'system', label: '跟随系统', icon: Monitor }
  ] as const;
</script>

<div role="group" aria-label="外观设置" class="flex rounded-lg bg-border/60 p-0.5">
  {#each modes as mode}
    <Button
      variant="ghost"
      size="icon-sm"
      class={userPrefersMode.current === mode.value
        ? 'flex-1 bg-background text-foreground shadow-sm hover:bg-background'
        : 'flex-1 text-muted-foreground'}
      aria-label={mode.label}
      aria-pressed={userPrefersMode.current === mode.value}
      onclick={() => setMode(mode.value)}
    >
      <mode.icon class="size-3.5" aria-hidden="true" />
    </Button>
  {/each}
</div>
