<script lang="ts">
  // Both applications use the same light/dark logo assets.
  import lightLogo from './assets/logo-light.svg';
  import darkLogo from './assets/logo-dark.svg';

  let { compact = false, collapsed = false, large = false, class: className = '' }: {
    compact?: boolean; collapsed?: boolean; large?: boolean; class?: string;
  } = $props();
</script>

<span class={`inline-flex shrink-0 items-center ${large ? 'gap-3' : 'gap-2'} ${className}`}>
  <span class={large ? 'size-12 shrink-0' : 'size-8 shrink-0'}>
    <img src={lightLogo} alt={compact ? 'Q2Q' : ''} class="size-full dark:hidden" />
    <img src={darkLogo} alt={compact ? 'Q2Q' : ''} class="hidden size-full dark:block" />
  </span>
  {#if !compact}<span aria-hidden={collapsed} class={`brand-name font-semibold tracking-tight transition-opacity duration-200 motion-reduce:transition-none ${collapsed ? 'opacity-0' : ''} ${large ? 'text-3xl' : 'text-xl'}`}>Q2Q</span>{/if}
</span>

<style>
  /* Match the light/dark hero title gradients in docs/.vitepress/theme/style.css. */
  .brand-name {
    background-image: -webkit-linear-gradient(120deg, #0d9488 10%, #2563eb 50%, #7c3aed 90%);
    background-clip: text;
    -webkit-background-clip: text;
    color: transparent;
  }
  :global(.dark) .brand-name {
    background-image: -webkit-linear-gradient(120deg, #2dd4bf 10%, #60a5fa 50%, #a78bfa 90%);
  }
</style>
