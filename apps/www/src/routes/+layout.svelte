<script lang="ts">
  import './layout.css';
  import { ModeWatcher } from 'mode-watcher';
  import favicon from '../../../../docs/public/favicon.svg';
  import type { LayoutProps } from './$types';
  let { children }: LayoutProps = $props();
</script>
<svelte:head><link rel="icon" href={favicon} /></svelte:head>
<ModeWatcher defaultMode="system" modeStorageKey="q2q-web-mode" />
{@render children()}
