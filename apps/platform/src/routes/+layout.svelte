<script lang="ts">
  import './layout.css';
  import favicon from '../../../../docs/public/favicon.svg';
  import { ModeWatcher } from 'mode-watcher';
  import AppShell from '#lib/components/app-shell.svelte';
  import type { LayoutProps } from './$types';

  let { data, children }: LayoutProps = $props();
</script>

<svelte:head>
  <link rel="icon" href={favicon} />
</svelte:head>

<ModeWatcher defaultMode="system" modeStorageKey="q2q-platform-mode" />
{#if data.user && data.currentProject && data.entitlements}
  <AppShell user={data.user} projects={data.projects} currentProject={data.currentProject} entitlements={data.entitlements}>
    {@render children()}
  </AppShell>
{:else}
  {@render children()}
{/if}
