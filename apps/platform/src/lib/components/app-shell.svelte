<script lang="ts">
  import type { Snippet } from 'svelte';
  import { MediaQuery } from 'svelte/reactivity';
  import { afterNavigate } from '$app/navigation';
  import { page } from '$app/state';
  import Brand from '#lib/components/brand.svelte';
  import LayoutDashboard from '@lucide/svelte/icons/layout-dashboard';
  import FolderClosed from '@lucide/svelte/icons/folder-closed';
  import Sprout from '@lucide/svelte/icons/sprout';
  import Network from '@lucide/svelte/icons/network';
  import HardDrive from '@lucide/svelte/icons/hard-drive';
  import Radio from '@lucide/svelte/icons/radio';
  import ArrowLeft from '@lucide/svelte/icons/arrow-left';
  import PanelLeft from '@lucide/svelte/icons/panel-left';
  import X from '@lucide/svelte/icons/x';
  import { messages } from '#lib/i18n.js';
  import { Button } from '#lib/components/ui/button/index.js';
  import * as Sheet from '#lib/components/ui/sheet/index.js';
  import ProjectPicker from '#lib/components/project-picker.svelte';
  import AccountMenu from '#lib/components/account-menu.svelte';
  import type { Project } from '#lib/server/projects.js';
  import type { ProjectEntitlements } from '#lib/server/plans.js';

  let { user, projects, currentProject, entitlements, children }: {
    user: { name: string; image: string | null };
    projects: Project[];
    currentProject: Project;
    entitlements: ProjectEntitlements;
    children: Snippet;
  } = $props();
  const text = $derived(messages[page.data.locale]);
  const section = $derived(page.url.pathname === '/' ? 'overview' : page.url.pathname.startsWith('/seeds') ? 'seeds' : page.url.pathname === '/network' ? 'network' : page.url.pathname === '/settings' ? 'settings' : page.url.pathname === '/storage' ? 'storage' : page.url.pathname === '/usage' ? 'usage' : page.url.pathname.startsWith('/project/') ? 'projectSettings' : 'library');
  const pageBack = $derived(page.url.pathname.startsWith('/library/')
    ? { href: '/library', label: text.backToLibrary }
    : page.url.pathname.startsWith('/seeds/')
      ? { href: '/seeds', label: text.seedBack }
      : null);
  const desktop = new MediaQuery('(min-width: 1024px)');
  const persistent = new MediaQuery('(min-width: 768px)');
  // Follow the viewport until the user chooses a width; resizing preserves that choice.
  let sidebarChoice = $state<boolean | null>(null);
  const expanded = $derived(sidebarChoice ?? desktop.current);
  let drawerOpen = $state(false);
  let drawerTrigger: HTMLButtonElement | null = null;
  const closeDrawer = () => { drawerOpen = false; };
  function openDrawer(event: MouseEvent) {
    drawerTrigger = event.currentTarget as HTMLButtonElement;
    drawerOpen = true;
  }
  afterNavigate(closeDrawer);
  // A drawer opened on mobile must not keep the desktop behind a modal after resizing.
  $effect(() => { if (persistent.current) drawerOpen = false; });

  const navigation = [
    { label: 'overview', icon: LayoutDashboard, href: '/' },
    { label: 'library', icon: FolderClosed, href: '/library' },
    { label: 'seeds', icon: Sprout, href: '/seeds' },
    { label: 'storage', icon: HardDrive, href: '/storage' },
    { label: 'network', icon: Network, href: '/network' },
    { label: 'nodes', icon: Radio, href: null }
  ] as const;
</script>

{#snippet fullSidebar(drawer = false, compact = false)}
  <div class={`flex min-h-0 flex-1 flex-col overflow-y-auto overscroll-contain ${compact ? 'gap-4 px-1 pt-4 pb-4' : 'gap-6 px-3 pt-6 pb-5'}`} style={`width: ${drawer ? '100%' : compact ? '64px' : '232px'};`}>
    <a href="/" onclick={drawer ? closeDrawer : undefined} class={`flex h-8 items-center text-xl font-medium tracking-tight ${compact ? 'justify-center px-0' : 'gap-2 px-2'}`} aria-label="Q2Q">
      <Brand compact={compact} collapsed={compact} />
    </a>
    <ProjectPicker {projects} {currentProject} {compact} />
    <div>
      <p class={`sidebar-label mb-2 px-3 text-xs text-muted-foreground ${compact ? 'hidden' : ''}`} aria-hidden={compact}>{text.workspace}</p>
      <nav aria-label={text.navigation} class="flex flex-col gap-1">
        {#each navigation as item}
          {@const active = item.href !== null && (page.url.pathname === item.href || page.url.pathname.startsWith(item.href + '/'))}
          <Button href={item.href ?? undefined} variant="ghost" disabled={item.href === null}
            onclick={drawer ? closeDrawer : undefined} aria-current={active ? 'page' : undefined}
            aria-label={text[item.label]} title={compact ? text[item.label] : undefined}
            style={`width: ${drawer ? '100%' : compact ? '40px' : '208px'};`}
            class={active
              ? `${compact ? 'mx-auto h-10 justify-center px-0' : 'h-11 justify-start gap-3 px-3.75'} transition-[width] duration-200 motion-reduce:transition-none bg-sidebar-accent text-sidebar-accent-foreground hover:bg-sidebar-accent`
              : `${compact ? 'mx-auto h-10 justify-center px-0' : 'h-11 justify-start gap-3 px-3.75'} transition-[width] duration-200 motion-reduce:transition-none text-muted-foreground`}>
            <item.icon class="size-4.5 shrink-0" aria-hidden="true" /><span class={`sidebar-label text-sm ${compact ? 'hidden' : ''}`} aria-hidden="true">{text[item.label]}</span>
          </Button>
        {/each}
      </nav>
    </div>
  </div>
  <div class="shrink-0 overflow-hidden transition-[width] duration-200 motion-reduce:transition-none" style={`width: ${drawer ? '100%' : compact ? '64px' : '232px'};`}><AccountMenu {user} planId={entitlements.planId} {compact} onNavigate={drawer ? closeDrawer : undefined} /></div>
{/snippet}

<Sheet.Root bind:open={drawerOpen}>
  <div class="app-layout min-h-dvh bg-background text-foreground" style={`--sidebar-width: ${expanded ? 232 : 64}px;`}>
    <aside aria-label={text.navigation} class="sticky top-0 hidden h-dvh min-h-0 flex-col overflow-x-clip border-r bg-sidebar md:flex" style="padding-bottom: env(safe-area-inset-bottom);">
      {@render fullSidebar(false, !expanded)}
    </aside>
    <main class="min-w-0 pb-[env(safe-area-inset-bottom)]">
      <header class="sticky top-0 z-20 flex min-h-14 items-center gap-3 border-b bg-background/90 px-3 backdrop-blur-xl supports-backdrop-filter:bg-background/80 md:min-h-14 md:px-6 lg:px-7" style="padding-top: env(safe-area-inset-top);">
        <div class={pageBack
          ? 'inline-flex h-10 shrink-0 items-center rounded-full border bg-card/80 p-1 shadow-sm'
          : 'inline-flex size-8 shrink-0 items-center justify-center rounded-lg border bg-card/80 shadow-sm'}>
          {#if pageBack}<Button href={pageBack.href} variant="ghost" size="icon-lg" class="size-8 rounded-lg" aria-label={pageBack.label}><ArrowLeft class="size-4.5" aria-hidden="true" /></Button><span class="mx-0.5 h-5 w-px bg-border" aria-hidden="true"></span>{/if}
          <Button variant="ghost" size="icon-lg" class="size-8 rounded-lg md:hidden" aria-label={text.openNavigation} aria-expanded={drawerOpen} aria-haspopup="dialog" onclick={openDrawer}><PanelLeft class="size-4.5" aria-hidden="true" /></Button>
          <Button variant="ghost" size="icon-lg" class="hidden size-8 rounded-lg md:inline-flex" aria-label={expanded ? text.collapseSidebar : text.expandSidebar} aria-expanded={expanded} onclick={() => sidebarChoice = !expanded}><PanelLeft class="size-4.5" aria-hidden="true" /></Button>
        </div>
        <p class="flex min-w-0 items-center gap-2 text-[13px] text-muted-foreground">
          <span class="truncate">{currentProject.personalUserId ? text.personalProject : currentProject.name}</span>
          <span aria-hidden="true">/</span><span class="shrink-0 text-foreground">{text[section]}</span>
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
    {@render fullSidebar(true)}
  </Sheet.Content>
</Sheet.Root>

<style>
  .sidebar-label { transition: opacity 180ms ease; white-space: nowrap; }
  .label-hidden { opacity: 0; pointer-events: none; }
  @media (min-width: 768px) {
    .app-layout { display: grid; grid-template-columns: var(--sidebar-width) minmax(0, 1fr); transition: grid-template-columns 240ms cubic-bezier(.2,.8,.2,1); }
  }
  @media (prefers-reduced-motion: reduce) {
    .app-layout, .sidebar-label { transition: none; }
  }
</style>
