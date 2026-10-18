<script lang="ts">
  import './layout.css';
  import favicon from '../../../../docs/public/favicon.svg';
  import { ModeWatcher } from 'mode-watcher';
  import { navigating } from '$app/state';
  import { messages } from '#lib/i18n.js';
  import AppShell from '#lib/components/app-shell.svelte';
  import type { LayoutProps } from './$types';

  let { data, children }: LayoutProps = $props();
  const text = $derived(messages[data.locale]);
  let showNavigationProgress = $state(false);

  $effect(() => {
    if (!navigating.to) {
      showNavigationProgress = false;
      return;
    }

    showNavigationProgress = false;
    const timer = setTimeout(() => { showNavigationProgress = true; }, 120);
    return () => clearTimeout(timer);
  });
</script>

<svelte:head>
  <link rel="icon" href={favicon} />
</svelte:head>

<ModeWatcher defaultMode="system" modeStorageKey="q2q-platform-mode" />
{#if showNavigationProgress}
  <div class="navigation-progress" role="progressbar" aria-label={text.navigationProgress}>
    <div class="navigation-progress-indicator"></div>
  </div>
{/if}
{#if data.user && data.currentProject && data.entitlements}
  <AppShell user={data.user} projects={data.projects} currentProject={data.currentProject} entitlements={data.entitlements}>
    {@render children()}
  </AppShell>
{:else}
  {@render children()}
{/if}

<style>
  .navigation-progress {
    position: fixed;
    inset: 0 0 auto;
    z-index: 50;
    height: 2px;
    overflow: hidden;
    background: color-mix(in srgb, var(--primary) 14%, transparent);
    pointer-events: none;
  }

  .navigation-progress-indicator {
    width: 30%;
    height: 100%;
    border-radius: 9999px;
    background: var(--primary);
    animation: navigation-progress 1.1s cubic-bezier(0.4, 0, 0.2, 1) infinite;
  }

  @keyframes navigation-progress {
    from { transform: translateX(-110%); }
    to { transform: translateX(440%); }
  }

  @media (prefers-reduced-motion: reduce) {
    .navigation-progress-indicator {
      width: 100%;
      animation: none;
    }
  }
</style>
