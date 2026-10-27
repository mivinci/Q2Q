<script lang="ts">
  import type { Snippet } from 'svelte';
  import { MediaQuery } from 'svelte/reactivity';
  import ArrowLeft from '@lucide/svelte/icons/arrow-left';
  import PanelLeft from '@lucide/svelte/icons/panel-left';
  import X from '@lucide/svelte/icons/x';
  import { Button } from './button/index.js';
  import * as Sheet from './sheet/index.js';

  let { labels: text, navigationKey, sidebar, breadcrumb, pageBack = null, children }: {
    labels: { navigation: string; navigationDescription: string; openNavigation: string; closeNavigation: string; collapseSidebar: string; expandSidebar: string };
    navigationKey: string;
    sidebar: Snippet<[compact: boolean, onNavigate: () => void]>;
    breadcrumb: Snippet;
    pageBack?: { href: string; label: string } | null;
    children: Snippet;
  } = $props();
  const desktop = new MediaQuery('(min-width: 1024px)');
  const persistent = new MediaQuery('(min-width: 768px)');
  let sidebarChoice = $state<boolean | null>(null);
  const expanded = $derived(sidebarChoice ?? desktop.current);
  let drawerOpen = $state(false);
  let drawerTrigger: HTMLButtonElement | null = null;
  const closeDrawer = () => { drawerOpen = false; };
  function openDrawer(event: MouseEvent) {
    drawerTrigger = event.currentTarget as HTMLButtonElement;
    drawerOpen = true;
  }
  // Consumers pass their navigation state; the shared layout does not import a router.
  $effect(() => { navigationKey; closeDrawer(); });
  $effect(() => { if (persistent.current) drawerOpen = false; });
</script>

<Sheet.Root bind:open={drawerOpen}>
  <div class="app-layout min-h-dvh bg-background text-foreground" style={`--sidebar-width: ${expanded ? 232 : 56}px;`}>
    <aside aria-label={text.navigation} class="sticky top-0 hidden h-dvh min-h-0 flex-col overflow-x-clip border-r bg-sidebar md:flex" style="padding-bottom: env(safe-area-inset-bottom);">
      {@render sidebar(!expanded, closeDrawer)}
    </aside>
    <main class="min-w-0 pb-[env(safe-area-inset-bottom)]">
      <header class="sticky top-0 z-20 flex min-h-14 items-center gap-3 border-b bg-background/90 px-3 backdrop-blur-xl supports-backdrop-filter:bg-background/80 md:min-h-14 md:px-6 lg:px-7" style="padding-top: env(safe-area-inset-top);">
        <div class={pageBack
          ? 'inline-flex h-10 shrink-0 items-center rounded-full border bg-card/80 p-1 shadow-xs'
          : 'inline-flex size-8 shrink-0 items-center justify-center rounded-lg border bg-card/80 shadow-xs'}>
          {#if pageBack}<Button href={pageBack.href} variant="ghost" size="icon-lg" class="size-8 rounded-full" aria-label={pageBack.label}><ArrowLeft class="size-4.5" aria-hidden="true" /></Button><span class="mx-0.5 h-5 w-px bg-border" aria-hidden="true"></span>{/if}
          <Button variant="ghost" size="icon-lg" class={`size-8 ${pageBack ? 'rounded-full' : 'rounded-lg'} md:hidden`} aria-label={text.openNavigation} aria-expanded={drawerOpen} aria-haspopup="dialog" onclick={openDrawer}><PanelLeft class="size-4.5" aria-hidden="true" /></Button>
          <Button variant="ghost" size="icon-lg" class={`hidden size-8 ${pageBack ? 'rounded-full' : 'rounded-lg'} md:inline-flex`} aria-label={expanded ? text.collapseSidebar : text.expandSidebar} aria-expanded={expanded} onclick={() => sidebarChoice = !expanded}><PanelLeft class="size-4.5" aria-hidden="true" /></Button>
        </div>
        <p class="flex min-w-0 items-center gap-2 text-[13px] text-muted-foreground">
          {@render breadcrumb()}
        </p>
      </header>
      {@render children()}
    </main>
  </div>
  <Sheet.Content side="left" showCloseButton={false}
    class="gap-0 bg-sidebar p-0 data-[side=left]:h-dvh data-[side=left]:w-[min(304px,calc(100vw-48px))] data-[side=left]:sm:max-w-none"
    style="padding-top: env(safe-area-inset-top); padding-bottom: env(safe-area-inset-bottom);"
    onCloseAutoFocus={(event) => { event.preventDefault(); drawerTrigger?.focus(); }}>
    <Sheet.Title class="sr-only">{text.navigation}</Sheet.Title>
    <Sheet.Description class="sr-only">{text.navigationDescription}</Sheet.Description>
    <Button variant="ghost" size="icon-lg" class="absolute top-3 right-3 size-11" aria-label={text.closeNavigation} onclick={closeDrawer}><X class="size-5" aria-hidden="true" /></Button>
    {@render sidebar(false, closeDrawer)}
  </Sheet.Content>
</Sheet.Root>

<style>
  @media (min-width: 768px) {
    .app-layout { display: grid; grid-template-columns: var(--sidebar-width) minmax(0, 1fr); transition: grid-template-columns 240ms cubic-bezier(.2,.8,.2,1); }
  }
  @media (prefers-reduced-motion: reduce) { .app-layout { transition: none; } }
</style>
