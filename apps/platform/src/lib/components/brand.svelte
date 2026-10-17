<script lang="ts">
  // Import the docs assets directly so both applications keep the same logo source.
  import lightLogo from '../../../../../docs/public/logo-light.svg';
  import darkLogo from '../../../../../docs/public/logo-dark.svg';

  let { compact = false, large = false, class: className = '' }: {
    compact?: boolean; large?: boolean; class?: string;
  } = $props();
</script>

<span class={`inline-flex shrink-0 items-center ${large ? 'gap-3' : 'gap-2'} ${className}`}>
  <span class={large ? 'size-12 shrink-0' : 'size-8 shrink-0'}>
    <img src={lightLogo} alt={compact ? 'Q2Q' : ''} class="size-full dark:hidden" />
    <img src={darkLogo} alt={compact ? 'Q2Q' : ''} class="hidden size-full dark:block" />
  </span>
  {#if !compact}<span class={`brand-name font-semibold tracking-tight ${large ? 'text-3xl' : 'text-xl'}`}>Q2Q</span>{/if}
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
